//! Compression and decompression for artifact bodies.
//!
//! This module provides lossless compression to reduce storage requirements
//! for artifact body bytes.  Two algorithms are offered in addition to the
//! `None` passthrough:
//!
//! - **Run-Length Encoding** ([`RunLengthCompressor`]) -- effective for data
//!   with repeated byte runs (e.g. zero-padded buffers, sparse matrices).
//! - **Dictionary Compression** ([`DictionaryCompressor`]) -- effective for
//!   JSON and other text artifacts that contain many repeated short strings
//!   (field names, punctuation patterns).
//!
//! [`auto_compress`] tries each algorithm and selects the one that achieves
//! the best compression ratio, falling back to `None` when neither algorithm
//! shrinks the input.
//!
//! # Round-trip guarantee
//!
//! Every [`Compressor`] implementation upholds the invariant that
//! `decompress(compress(data)) == data` for all inputs, including empty
//! slices and arbitrary binary content.

use std::collections::HashMap;
use std::fmt;
use std::time::Instant;

// ---------------------------------------------------------------------------
// CompressionAlgorithm
// ---------------------------------------------------------------------------

/// Identifies the algorithm used to produce a [`CompressedBody`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CompressionAlgorithm {
    /// No compression -- body is stored verbatim.
    None,
    /// Run-length encoding.
    RunLength,
    /// Dictionary-based compression for text/JSON payloads.
    Dictionary,
}

impl fmt::Display for CompressionAlgorithm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => write!(f, "none"),
            Self::RunLength => write!(f, "run-length"),
            Self::Dictionary => write!(f, "dictionary"),
        }
    }
}

// ---------------------------------------------------------------------------
// CompressError
// ---------------------------------------------------------------------------

/// Errors that may occur during compression or decompression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompressError {
    /// The compressed payload is malformed or truncated.
    InvalidData(String),
    /// Decompressed output size does not match the recorded `original_size`.
    SizeMismatch {
        expected: usize,
        actual: usize,
    },
}

impl fmt::Display for CompressError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidData(msg) => write!(f, "invalid compressed data: {msg}"),
            Self::SizeMismatch { expected, actual } => {
                write!(
                    f,
                    "decompressed size mismatch: expected {expected}, got {actual}"
                )
            }
        }
    }
}

impl std::error::Error for CompressError {}

/// Convenience alias for compression results.
pub type CompressResult<T> = Result<T, CompressError>;

// ---------------------------------------------------------------------------
// CompressedBody
// ---------------------------------------------------------------------------

/// A compressed artifact body together with the metadata needed to
/// decompress and verify it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompressedBody {
    /// Algorithm that was used.
    pub algorithm: CompressionAlgorithm,
    /// Size of the original, uncompressed data in bytes.
    pub original_size: usize,
    /// Size of `data` in bytes (equal to `data.len()`).
    pub compressed_size: usize,
    /// The compressed (or verbatim) bytes.
    pub data: Vec<u8>,
}

impl CompressedBody {
    /// Compression ratio as `compressed_size / original_size`.
    ///
    /// Returns `1.0` when the original size is zero to avoid division by zero.
    #[must_use]
    pub fn ratio(&self) -> f64 {
        if self.original_size == 0 {
            1.0
        } else {
            self.compressed_size as f64 / self.original_size as f64
        }
    }
}

// ---------------------------------------------------------------------------
// CompressionStats
// ---------------------------------------------------------------------------

/// Summary statistics for a compression operation.
#[derive(Debug, Clone)]
pub struct CompressionStats {
    /// Size of the input data in bytes.
    pub original_size: usize,
    /// Size of the compressed output in bytes.
    pub compressed_size: usize,
    /// Compression ratio (`compressed / original`).  Values below `1.0`
    /// indicate that the data shrank.
    pub ratio: f64,
    /// Algorithm that was used.
    pub algorithm: CompressionAlgorithm,
    /// Wall-clock time spent compressing, in microseconds.
    pub duration_us: u64,
}

// ---------------------------------------------------------------------------
// Compressor trait
// ---------------------------------------------------------------------------

/// A lossless compressor/decompressor pair.
///
/// Implementations must guarantee that for all byte slices `data`:
///
/// ```text
/// decompress(compress(data)?) == Ok(data.to_vec())
/// ```
pub trait Compressor {
    /// Compress `data` and return a [`CompressedBody`].
    fn compress(&self, data: &[u8]) -> CompressResult<CompressedBody>;

    /// Decompress a [`CompressedBody`] back to the original bytes.
    fn decompress(&self, body: &CompressedBody) -> CompressResult<Vec<u8>>;
}

// ---------------------------------------------------------------------------
// NoneCompressor
// ---------------------------------------------------------------------------

/// Passthrough "compressor" that stores data verbatim.
#[derive(Debug, Default)]
pub struct NoneCompressor;

impl Compressor for NoneCompressor {
    fn compress(&self, data: &[u8]) -> CompressResult<CompressedBody> {
        Ok(CompressedBody {
            algorithm: CompressionAlgorithm::None,
            original_size: data.len(),
            compressed_size: data.len(),
            data: data.to_vec(),
        })
    }

    fn decompress(&self, body: &CompressedBody) -> CompressResult<Vec<u8>> {
        if body.data.len() != body.original_size {
            return Err(CompressError::SizeMismatch {
                expected: body.original_size,
                actual: body.data.len(),
            });
        }
        Ok(body.data.clone())
    }
}

// ---------------------------------------------------------------------------
// RunLengthCompressor
// ---------------------------------------------------------------------------

/// Run-length encoding compressor.
///
/// Encoding format: a sequence of `(count, byte)` pairs.
///
/// - If `count <= 3` the pair is stored as the raw bytes repeated (no header),
///   **unless** the byte value equals the escape byte (`0xFF`), in which case
///   the escaped form is always used.
/// - If `count >= 4` (or the byte is `0xFF`) the pair is stored as
///   `[0xFF, count_high, count_low, byte]` where `count` is a big-endian
///   `u16`.  This supports runs up to 65 535 bytes.
///
/// The escape byte `0xFF` at the start of a group always signals an encoded
/// run.  Literal `0xFF` bytes in the input are encoded as `[0xFF, 0x00, 0x01, 0xFF]`.
#[derive(Debug, Default)]
pub struct RunLengthCompressor;

/// Escape byte used by [`RunLengthCompressor`].
const RLE_ESCAPE: u8 = 0xFF;

impl RunLengthCompressor {
    /// Maximum run length representable in a single encoded group.
    const MAX_RUN: usize = u16::MAX as usize;
}

impl Compressor for RunLengthCompressor {
    fn compress(&self, data: &[u8]) -> CompressResult<CompressedBody> {
        let mut out = Vec::with_capacity(data.len());

        let mut i = 0;
        while i < data.len() {
            let byte = data[i];
            let mut run_len: usize = 1;
            while i + run_len < data.len()
                && data[i + run_len] == byte
                && run_len < Self::MAX_RUN
            {
                run_len += 1;
            }

            if byte == RLE_ESCAPE || run_len >= 4 {
                // Encoded run: [escape, count_hi, count_lo, byte]
                let count = run_len as u16;
                out.push(RLE_ESCAPE);
                out.push((count >> 8) as u8);
                out.push((count & 0xFF) as u8);
                out.push(byte);
            } else {
                // Literal bytes (1..=3 copies, byte != escape)
                for _ in 0..run_len {
                    out.push(byte);
                }
            }
            i += run_len;
        }

        Ok(CompressedBody {
            algorithm: CompressionAlgorithm::RunLength,
            original_size: data.len(),
            compressed_size: out.len(),
            data: out,
        })
    }

    fn decompress(&self, body: &CompressedBody) -> CompressResult<Vec<u8>> {
        let src = &body.data;
        let mut out = Vec::with_capacity(body.original_size);
        let mut i = 0;

        while i < src.len() {
            if src[i] == RLE_ESCAPE {
                // Encoded run
                if i + 3 >= src.len() {
                    return Err(CompressError::InvalidData(
                        "truncated RLE escape sequence".into(),
                    ));
                }
                let count = ((src[i + 1] as usize) << 8) | (src[i + 2] as usize);
                let byte = src[i + 3];
                for _ in 0..count {
                    out.push(byte);
                }
                i += 4;
            } else {
                // Literal byte
                out.push(src[i]);
                i += 1;
            }
        }

        if out.len() != body.original_size {
            return Err(CompressError::SizeMismatch {
                expected: body.original_size,
                actual: out.len(),
            });
        }
        Ok(out)
    }
}

// ---------------------------------------------------------------------------
// DictionaryCompressor
// ---------------------------------------------------------------------------

/// Dictionary-based compressor optimized for JSON-like text payloads.
///
/// # Encoding format
///
/// The compressed output starts with a header:
///
/// 1. **Magic** -- two bytes `[0xDC, 0x01]` (Dictionary Compression v1).
/// 2. **Entry count** -- `u16` big-endian, the number of dictionary entries.
/// 3. **Entries** -- for each entry:
///    - `u8` code (starting from `0x01`).
///    - `u8` length of the phrase.
///    - `[u8; length]` the phrase bytes.
/// 4. **Compressed body** -- the input data with every occurrence of a
///    dictionary phrase replaced by the two-byte sequence `[0x00, code]`.
///    Literal `0x00` bytes in the input are escaped as `[0x00, 0x00]`.
///
/// The dictionary is built at compression time by counting all substrings of
/// length 2..=32 that appear at least twice and selecting the phrases that
/// yield the greatest byte savings.
#[derive(Debug, Default)]
pub struct DictionaryCompressor;

/// Magic bytes for the dictionary compression format.
const DICT_MAGIC: [u8; 2] = [0xDC, 0x01];

/// Escape byte for dictionary-encoded output.
const DICT_ESCAPE: u8 = 0x00;

/// Maximum number of dictionary entries (codes `0x01..=0xFE`).
const MAX_DICT_ENTRIES: usize = 254;

/// Minimum phrase length considered for the dictionary.
const MIN_PHRASE_LEN: usize = 2;

/// Maximum phrase length considered for the dictionary.
const MAX_PHRASE_LEN: usize = 32;

impl DictionaryCompressor {
    /// Build a dictionary from `data` by scoring candidate phrases.
    ///
    /// Returns a list of `(phrase, code)` pairs sorted by descending savings.
    fn build_dictionary(data: &[u8]) -> Vec<(Vec<u8>, u8)> {
        if data.len() < MIN_PHRASE_LEN {
            return Vec::new();
        }

        // Count occurrences of candidate phrases.
        let mut counts: HashMap<&[u8], usize> = HashMap::new();
        for phrase_len in MIN_PHRASE_LEN..=MAX_PHRASE_LEN.min(data.len()) {
            for window in data.windows(phrase_len) {
                *counts.entry(window).or_insert(0) += 1;
            }
        }

        // Score each phrase: savings = occurrences * (phrase_len - 2) - header_cost
        // where header_cost = 2 + phrase_len (code byte + length byte + phrase).
        // We only keep phrases with positive net savings.
        let mut candidates: Vec<(&[u8], i64)> = counts
            .into_iter()
            .filter_map(|(phrase, count)| {
                let replacement_savings =
                    (count as i64) * (phrase.len() as i64 - 2);
                let header_cost = 2 + phrase.len() as i64;
                let net = replacement_savings - header_cost;
                if net > 0 { Some((phrase, net)) } else { None }
            })
            .collect();

        // Sort by descending savings, then by shorter phrase (tie-break).
        candidates.sort_by(|a, b| {
            b.1.cmp(&a.1)
                .then_with(|| a.0.len().cmp(&b.0.len()))
        });

        // Take the top entries and assign codes starting from 0x01.
        candidates
            .into_iter()
            .take(MAX_DICT_ENTRIES)
            .enumerate()
            .map(|(i, (phrase, _))| (phrase.to_vec(), (i + 1) as u8))
            .collect()
    }

    /// Serialize the dictionary header into `out`.
    fn write_header(entries: &[(Vec<u8>, u8)], out: &mut Vec<u8>) {
        out.extend_from_slice(&DICT_MAGIC);
        let count = entries.len() as u16;
        out.push((count >> 8) as u8);
        out.push((count & 0xFF) as u8);
        for (phrase, code) in entries {
            out.push(*code);
            out.push(phrase.len() as u8);
            out.extend_from_slice(phrase);
        }
    }

    /// Parse the dictionary header from `src`, returning the entries and the
    /// offset where the compressed body begins.
    fn read_header(src: &[u8]) -> CompressResult<(Vec<(Vec<u8>, u8)>, usize)> {
        if src.len() < 4 {
            return Err(CompressError::InvalidData(
                "dictionary header too short".into(),
            ));
        }
        if src[0..2] != DICT_MAGIC {
            return Err(CompressError::InvalidData(
                "bad dictionary magic bytes".into(),
            ));
        }
        let entry_count = ((src[2] as usize) << 8) | (src[3] as usize);
        let mut entries = Vec::with_capacity(entry_count);
        let mut offset = 4;

        for _ in 0..entry_count {
            if offset + 2 > src.len() {
                return Err(CompressError::InvalidData(
                    "truncated dictionary entry".into(),
                ));
            }
            let code = src[offset];
            let len = src[offset + 1] as usize;
            offset += 2;
            if offset + len > src.len() {
                return Err(CompressError::InvalidData(
                    "truncated dictionary phrase".into(),
                ));
            }
            let phrase = src[offset..offset + len].to_vec();
            offset += len;
            entries.push((phrase, code));
        }

        Ok((entries, offset))
    }
}

impl Compressor for DictionaryCompressor {
    fn compress(&self, data: &[u8]) -> CompressResult<CompressedBody> {
        let entries = Self::build_dictionary(data);

        let mut out = Vec::with_capacity(data.len());
        Self::write_header(&entries, &mut out);

        // Build a lookup sorted by descending phrase length so that longer
        // matches take priority.
        let mut sorted_entries = entries.clone();
        sorted_entries.sort_by(|a, b| b.0.len().cmp(&a.0.len()));

        // Replace phrases in the input.
        let mut i = 0;
        while i < data.len() {
            let mut matched = false;
            for (phrase, code) in &sorted_entries {
                if i + phrase.len() <= data.len()
                    && &data[i..i + phrase.len()] == phrase.as_slice()
                {
                    out.push(DICT_ESCAPE);
                    out.push(*code);
                    i += phrase.len();
                    matched = true;
                    break;
                }
            }
            if !matched {
                if data[i] == DICT_ESCAPE {
                    // Escape literal 0x00
                    out.push(DICT_ESCAPE);
                    out.push(DICT_ESCAPE);
                } else {
                    out.push(data[i]);
                }
                i += 1;
            }
        }

        Ok(CompressedBody {
            algorithm: CompressionAlgorithm::Dictionary,
            original_size: data.len(),
            compressed_size: out.len(),
            data: out,
        })
    }

    fn decompress(&self, body: &CompressedBody) -> CompressResult<Vec<u8>> {
        let (entries, body_offset) = Self::read_header(&body.data)?;

        // Build code -> phrase map.
        let lookup: HashMap<u8, &[u8]> = entries
            .iter()
            .map(|(phrase, code)| (*code, phrase.as_slice()))
            .collect();

        let src = &body.data[body_offset..];
        let mut out = Vec::with_capacity(body.original_size);
        let mut i = 0;

        while i < src.len() {
            if src[i] == DICT_ESCAPE {
                if i + 1 >= src.len() {
                    return Err(CompressError::InvalidData(
                        "truncated escape sequence in body".into(),
                    ));
                }
                let code = src[i + 1];
                if code == DICT_ESCAPE {
                    // Escaped literal 0x00
                    out.push(DICT_ESCAPE);
                } else if let Some(phrase) = lookup.get(&code) {
                    out.extend_from_slice(phrase);
                } else {
                    return Err(CompressError::InvalidData(format!(
                        "unknown dictionary code: {code:#04X}"
                    )));
                }
                i += 2;
            } else {
                out.push(src[i]);
                i += 1;
            }
        }

        if out.len() != body.original_size {
            return Err(CompressError::SizeMismatch {
                expected: body.original_size,
                actual: out.len(),
            });
        }
        Ok(out)
    }
}

// ---------------------------------------------------------------------------
// Public helpers
// ---------------------------------------------------------------------------

/// Compress `data` with the given algorithm.
pub fn compress(
    data: &[u8],
    algorithm: CompressionAlgorithm,
) -> CompressResult<CompressedBody> {
    match algorithm {
        CompressionAlgorithm::None => NoneCompressor.compress(data),
        CompressionAlgorithm::RunLength => RunLengthCompressor.compress(data),
        CompressionAlgorithm::Dictionary => DictionaryCompressor.compress(data),
    }
}

/// Decompress a [`CompressedBody`] back to the original bytes.
pub fn decompress(body: &CompressedBody) -> CompressResult<Vec<u8>> {
    match body.algorithm {
        CompressionAlgorithm::None => NoneCompressor.decompress(body),
        CompressionAlgorithm::RunLength => RunLengthCompressor.decompress(body),
        CompressionAlgorithm::Dictionary => DictionaryCompressor.decompress(body),
    }
}

/// Compress `data` with the given algorithm and return [`CompressionStats`].
pub fn compress_with_stats(
    data: &[u8],
    algorithm: CompressionAlgorithm,
) -> CompressResult<(CompressedBody, CompressionStats)> {
    let start = Instant::now();
    let body = compress(data, algorithm)?;
    let elapsed = start.elapsed();
    let stats = CompressionStats {
        original_size: body.original_size,
        compressed_size: body.compressed_size,
        ratio: body.ratio(),
        algorithm: body.algorithm,
        duration_us: elapsed.as_micros() as u64,
    };
    Ok((body, stats))
}

/// Try every available algorithm and return the result with the best
/// (smallest) compression ratio.
///
/// When no algorithm reduces the size, `CompressionAlgorithm::None` is
/// returned so the caller always gets a valid [`CompressedBody`].
pub fn auto_compress(data: &[u8]) -> CompressResult<CompressedBody> {
    let algorithms = [
        CompressionAlgorithm::None,
        CompressionAlgorithm::RunLength,
        CompressionAlgorithm::Dictionary,
    ];

    let mut best: Option<CompressedBody> = Option::None;

    for algo in &algorithms {
        let body = compress(data, *algo)?;
        let is_better = match &best {
            Some(current) => body.compressed_size < current.compressed_size,
            Option::None => true,
        };
        if is_better {
            best = Some(body);
        }
    }

    // Safety: `algorithms` is non-empty, so `best` is always `Some`.
    Ok(best.unwrap())
}

/// Like [`auto_compress`] but also returns [`CompressionStats`] for the
/// winning algorithm.
pub fn auto_compress_with_stats(
    data: &[u8],
) -> CompressResult<(CompressedBody, CompressionStats)> {
    let start = Instant::now();
    let body = auto_compress(data)?;
    let elapsed = start.elapsed();
    let stats = CompressionStats {
        original_size: body.original_size,
        compressed_size: body.compressed_size,
        ratio: body.ratio(),
        algorithm: body.algorithm,
        duration_us: elapsed.as_micros() as u64,
    };
    Ok((body, stats))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- Helpers ------------------------------------------------------------

    /// Assert that compressing and decompressing `data` with `algo` yields
    /// back the original bytes.
    fn assert_round_trip(data: &[u8], algo: CompressionAlgorithm) {
        let compressed = compress(data, algo)
            .unwrap_or_else(|e| panic!("compress({algo}) failed: {e}"));
        assert_eq!(compressed.original_size, data.len());
        assert_eq!(compressed.compressed_size, compressed.data.len());
        let decompressed = decompress(&compressed)
            .unwrap_or_else(|e| panic!("decompress({algo}) failed: {e}"));
        assert_eq!(
            decompressed, data,
            "round-trip failed for {algo} on {} bytes",
            data.len()
        );
    }

    // -- Round-trip: NoneCompressor -----------------------------------------

    #[test]
    fn none_round_trip_empty() {
        assert_round_trip(b"", CompressionAlgorithm::None);
    }

    #[test]
    fn none_round_trip_hello() {
        assert_round_trip(b"hello, world", CompressionAlgorithm::None);
    }

    #[test]
    fn none_round_trip_binary() {
        let data: Vec<u8> = (0..=255).collect();
        assert_round_trip(&data, CompressionAlgorithm::None);
    }

    // -- Round-trip: RunLengthCompressor ------------------------------------

    #[test]
    fn rle_round_trip_empty() {
        assert_round_trip(b"", CompressionAlgorithm::RunLength);
    }

    #[test]
    fn rle_round_trip_no_runs() {
        assert_round_trip(b"abcdef", CompressionAlgorithm::RunLength);
    }

    #[test]
    fn rle_round_trip_short_runs() {
        // Runs of 1, 2, 3 should stay literal (except escape byte).
        assert_round_trip(b"aabbccc", CompressionAlgorithm::RunLength);
    }

    #[test]
    fn rle_round_trip_long_run() {
        let data = vec![0x42; 1000];
        assert_round_trip(&data, CompressionAlgorithm::RunLength);
    }

    #[test]
    fn rle_round_trip_all_zeros() {
        let data = vec![0x00; 512];
        assert_round_trip(&data, CompressionAlgorithm::RunLength);
    }

    #[test]
    fn rle_round_trip_escape_byte() {
        // Data containing the escape byte 0xFF.
        let data = vec![RLE_ESCAPE; 10];
        assert_round_trip(&data, CompressionAlgorithm::RunLength);
    }

    #[test]
    fn rle_round_trip_mixed_with_escape() {
        let mut data = vec![0x41; 5];
        data.push(RLE_ESCAPE);
        data.extend_from_slice(&[0x42; 3]);
        data.push(RLE_ESCAPE);
        data.push(RLE_ESCAPE);
        data.extend_from_slice(b"end");
        assert_round_trip(&data, CompressionAlgorithm::RunLength);
    }

    #[test]
    fn rle_round_trip_single_byte() {
        assert_round_trip(&[0x7A], CompressionAlgorithm::RunLength);
    }

    #[test]
    fn rle_round_trip_binary_all_values() {
        let data: Vec<u8> = (0..=255).collect();
        assert_round_trip(&data, CompressionAlgorithm::RunLength);
    }

    #[test]
    fn rle_compresses_repeated_data() {
        let data = vec![0x00; 1024];
        let compressed =
            compress(&data, CompressionAlgorithm::RunLength).unwrap();
        assert!(
            compressed.compressed_size < data.len(),
            "RLE should compress repeated data: {} >= {}",
            compressed.compressed_size,
            data.len()
        );
    }

    // -- Round-trip: DictionaryCompressor -----------------------------------

    #[test]
    fn dict_round_trip_empty() {
        assert_round_trip(b"", CompressionAlgorithm::Dictionary);
    }

    #[test]
    fn dict_round_trip_short() {
        assert_round_trip(b"hi", CompressionAlgorithm::Dictionary);
    }

    #[test]
    fn dict_round_trip_json() {
        let json = br#"{"name":"alice","age":30,"name":"bob","age":25}"#;
        assert_round_trip(json, CompressionAlgorithm::Dictionary);
    }

    #[test]
    fn dict_round_trip_repeated_fields() {
        let json = br#"[{"type":"artifact","id":"1"},{"type":"artifact","id":"2"},{"type":"artifact","id":"3"}]"#;
        assert_round_trip(json, CompressionAlgorithm::Dictionary);
    }

    #[test]
    fn dict_round_trip_with_null_bytes() {
        // Data containing literal 0x00 bytes that must be escaped.
        let data: Vec<u8> = vec![0x00, 0x41, 0x00, 0x00, 0x42, 0x00];
        assert_round_trip(&data, CompressionAlgorithm::Dictionary);
    }

    #[test]
    fn dict_round_trip_binary_all_values() {
        let data: Vec<u8> = (0..=255).collect();
        assert_round_trip(&data, CompressionAlgorithm::Dictionary);
    }

    #[test]
    fn dict_round_trip_single_byte() {
        assert_round_trip(&[0x00], CompressionAlgorithm::Dictionary);
    }

    #[test]
    fn dict_compresses_repetitive_json() {
        let json = br#"{"field":"value","field":"value","field":"value","field":"value"}"#;
        let compressed =
            compress(json, CompressionAlgorithm::Dictionary).unwrap();
        assert!(
            compressed.compressed_size < json.len(),
            "Dictionary should compress repetitive JSON: {} >= {}",
            compressed.compressed_size,
            json.len()
        );
    }

    // -- auto_compress ------------------------------------------------------

    #[test]
    fn auto_compress_empty() {
        let body = auto_compress(b"").unwrap();
        let decompressed = decompress(&body).unwrap();
        assert_eq!(decompressed, b"");
    }

    #[test]
    fn auto_compress_selects_best() {
        // Highly repetitive data should prefer RLE.
        let data = vec![0x42; 2048];
        let body = auto_compress(&data).unwrap();
        assert_eq!(body.algorithm, CompressionAlgorithm::RunLength);
        let decompressed = decompress(&body).unwrap();
        assert_eq!(decompressed, data);
    }

    #[test]
    fn auto_compress_round_trip_text() {
        let text = b"the quick brown fox jumps over the lazy dog";
        let body = auto_compress(text).unwrap();
        let decompressed = decompress(&body).unwrap();
        assert_eq!(decompressed, text);
    }

    #[test]
    fn auto_compress_round_trip_random_looking() {
        // Pseudo-random data that is hard to compress.
        let data: Vec<u8> = (0..256).map(|i| ((i * 37 + 13) % 256) as u8).collect();
        let body = auto_compress(&data).unwrap();
        let decompressed = decompress(&body).unwrap();
        assert_eq!(decompressed, data);
    }

    // -- CompressionStats ---------------------------------------------------

    #[test]
    fn stats_original_and_compressed_sizes() {
        let data = b"stats test data here";
        let (body, stats) =
            compress_with_stats(data, CompressionAlgorithm::None).unwrap();
        assert_eq!(stats.original_size, data.len());
        assert_eq!(stats.compressed_size, body.data.len());
        assert_eq!(stats.algorithm, CompressionAlgorithm::None);
    }

    #[test]
    fn stats_ratio_identity_for_none() {
        let data = b"ratio test";
        let (_, stats) =
            compress_with_stats(data, CompressionAlgorithm::None).unwrap();
        assert!(
            (stats.ratio - 1.0).abs() < f64::EPSILON,
            "None ratio should be 1.0, got {}",
            stats.ratio
        );
    }

    #[test]
    fn stats_ratio_below_one_for_compressible() {
        let data = vec![0x00; 1024];
        let (_, stats) =
            compress_with_stats(&data, CompressionAlgorithm::RunLength).unwrap();
        assert!(
            stats.ratio < 1.0,
            "RLE on zeros should have ratio < 1.0, got {}",
            stats.ratio
        );
    }

    #[test]
    fn auto_compress_with_stats_returns_valid_stats() {
        let data = vec![0x41; 512];
        let (body, stats) = auto_compress_with_stats(&data).unwrap();
        assert_eq!(stats.original_size, data.len());
        assert_eq!(stats.compressed_size, body.compressed_size);
        assert!(stats.ratio <= 1.0);
        let decompressed = decompress(&body).unwrap();
        assert_eq!(decompressed, data);
    }

    // -- Edge cases ---------------------------------------------------------

    #[test]
    fn compressed_body_ratio_empty() {
        let body = CompressedBody {
            algorithm: CompressionAlgorithm::None,
            original_size: 0,
            compressed_size: 0,
            data: Vec::new(),
        };
        assert!((body.ratio() - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn rle_large_data_round_trip() {
        // 64 KiB of patterned data.
        let data: Vec<u8> = (0..65_536)
            .map(|i| if i % 100 < 80 { 0xAA } else { (i % 256) as u8 })
            .collect();
        assert_round_trip(&data, CompressionAlgorithm::RunLength);
    }

    #[test]
    fn dict_large_json_round_trip() {
        // Build a large JSON-like string with many repeated keys.
        let mut json = String::from("[");
        for i in 0..200 {
            if i > 0 {
                json.push(',');
            }
            json.push_str(&format!(
                r#"{{"id":{},"type":"artifact","status":"active"}}"#,
                i
            ));
        }
        json.push(']');
        assert_round_trip(json.as_bytes(), CompressionAlgorithm::Dictionary);
    }

    #[test]
    fn decompress_truncated_rle_returns_error() {
        // Construct a truncated RLE body.
        let body = CompressedBody {
            algorithm: CompressionAlgorithm::RunLength,
            original_size: 10,
            compressed_size: 2,
            data: vec![RLE_ESCAPE, 0x00], // truncated escape
        };
        let err = decompress(&body).unwrap_err();
        assert!(matches!(err, CompressError::InvalidData(_)));
    }

    #[test]
    fn decompress_bad_dict_magic_returns_error() {
        let body = CompressedBody {
            algorithm: CompressionAlgorithm::Dictionary,
            original_size: 5,
            compressed_size: 4,
            data: vec![0xBA, 0xAD, 0x00, 0x00],
        };
        let err = decompress(&body).unwrap_err();
        assert!(matches!(err, CompressError::InvalidData(_)));
    }

    #[test]
    fn decompress_none_size_mismatch_returns_error() {
        let body = CompressedBody {
            algorithm: CompressionAlgorithm::None,
            original_size: 10,
            compressed_size: 3,
            data: vec![1, 2, 3],
        };
        let err = decompress(&body).unwrap_err();
        assert!(matches!(err, CompressError::SizeMismatch { .. }));
    }

    #[test]
    fn algorithm_display() {
        assert_eq!(CompressionAlgorithm::None.to_string(), "none");
        assert_eq!(CompressionAlgorithm::RunLength.to_string(), "run-length");
        assert_eq!(CompressionAlgorithm::Dictionary.to_string(), "dictionary");
    }

    #[test]
    fn compress_error_display() {
        let e = CompressError::InvalidData("bad data".into());
        assert_eq!(e.to_string(), "invalid compressed data: bad data");

        let e = CompressError::SizeMismatch {
            expected: 10,
            actual: 5,
        };
        assert_eq!(
            e.to_string(),
            "decompressed size mismatch: expected 10, got 5"
        );
    }

    #[test]
    fn rle_max_run_boundary() {
        // A run exactly at the u16::MAX boundary.
        let data = vec![0x42; u16::MAX as usize];
        assert_round_trip(&data, CompressionAlgorithm::RunLength);
    }

    #[test]
    fn rle_exceeds_max_run() {
        // A run longer than u16::MAX should be split into multiple groups.
        let data = vec![0x42; (u16::MAX as usize) + 100];
        assert_round_trip(&data, CompressionAlgorithm::RunLength);
    }

    #[test]
    fn dict_unknown_code_returns_error() {
        // Valid header with zero entries, but body references code 0x05.
        let mut data = Vec::new();
        data.extend_from_slice(&DICT_MAGIC);
        data.push(0x00); // entry count high
        data.push(0x00); // entry count low
        data.push(DICT_ESCAPE);
        data.push(0x05); // unknown code

        let body = CompressedBody {
            algorithm: CompressionAlgorithm::Dictionary,
            original_size: 1,
            compressed_size: data.len(),
            data,
        };
        let err = decompress(&body).unwrap_err();
        assert!(matches!(err, CompressError::InvalidData(_)));
    }

    #[test]
    fn all_algorithms_agree_on_empty() {
        for algo in [
            CompressionAlgorithm::None,
            CompressionAlgorithm::RunLength,
            CompressionAlgorithm::Dictionary,
        ] {
            let body = compress(b"", algo).unwrap();
            let out = decompress(&body).unwrap();
            assert!(out.is_empty(), "{algo} did not round-trip empty data");
        }
    }
}
