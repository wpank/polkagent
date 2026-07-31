//! Core SCALE codec primitives.
//!
//! Implements the [SCALE (Simple Concatenated Aggregate Little-Endian)][scale]
//! codec as used by Substrate/Polkadot, without any dependency on
//! `parity-scale-codec` or `subxt`.
//!
//! [scale]: https://docs.substrate.io/reference/scale-codec/

use crate::error::{CodecError, Result};

// ---------------------------------------------------------------------------
// Decoder
// ---------------------------------------------------------------------------

/// A cursor-based decoder for SCALE-encoded byte slices.
///
/// The decoder tracks its position in the input and advances it as bytes are
/// consumed. All integer types are decoded as little-endian, matching the
/// Substrate SCALE specification.
pub struct ScaleDecoder<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> ScaleDecoder<'a> {
    /// Create a new decoder wrapping the given byte slice.
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    /// Return the current byte offset (number of bytes consumed so far).
    pub fn position(&self) -> usize {
        self.pos
    }

    /// Return the number of bytes remaining in the input.
    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    /// Return `true` if all input bytes have been consumed.
    pub fn is_empty(&self) -> bool {
        self.pos >= self.data.len()
    }

    /// Return a slice of the remaining (not yet consumed) bytes.
    pub fn remaining_bytes(&self) -> &[u8] {
        &self.data[self.pos..]
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    fn read_bytes(&mut self, n: usize) -> Result<&[u8]> {
        let end = self.pos + n;
        if end > self.data.len() {
            return Err(CodecError::unexpected_eof(self.pos));
        }
        let slice = &self.data[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    fn read_byte(&mut self) -> Result<u8> {
        if self.pos >= self.data.len() {
            return Err(CodecError::unexpected_eof(self.pos));
        }
        let b = self.data[self.pos];
        self.pos += 1;
        Ok(b)
    }

    // -----------------------------------------------------------------------
    // Primitive decoders
    // -----------------------------------------------------------------------

    /// Decode a single `u8`.
    pub fn decode_u8(&mut self) -> Result<u8> {
        self.read_byte()
    }

    /// Decode a little-endian `u16`.
    pub fn decode_u16(&mut self) -> Result<u16> {
        let bytes = self.read_bytes(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    /// Decode a little-endian `u32`.
    pub fn decode_u32(&mut self) -> Result<u32> {
        let bytes = self.read_bytes(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    /// Decode a little-endian `u64`.
    pub fn decode_u64(&mut self) -> Result<u64> {
        let bytes = self.read_bytes(8)?;
        let arr: [u8; 8] = bytes.try_into().map_err(|_| CodecError::decode("u64 slice conversion failed"))?;
        Ok(u64::from_le_bytes(arr))
    }

    /// Decode a little-endian `u128`.
    pub fn decode_u128(&mut self) -> Result<u128> {
        let bytes = self.read_bytes(16)?;
        let arr: [u8; 16] = bytes.try_into().map_err(|_| CodecError::decode("u128 slice conversion failed"))?;
        Ok(u128::from_le_bytes(arr))
    }

    /// Decode a SCALE boolean (`0x00` = false, `0x01` = true).
    pub fn decode_bool(&mut self) -> Result<bool> {
        match self.read_byte()? {
            0x00 => Ok(false),
            0x01 => Ok(true),
            b => Err(CodecError::invalid_type(format!(
                "invalid bool byte: 0x{b:02x}"
            ))),
        }
    }

    /// Decode a SCALE compact-encoded `u32`.
    ///
    /// The Substrate SCALE compact encoding uses the two LSBs of the first
    /// byte as a mode selector:
    ///
    /// | mode | first-byte LSBs | value range          |
    /// |------|-----------------|----------------------|
    /// | 0    | `0b00`          | 0 … 63               |
    /// | 1    | `0b01`          | 64 … 16383           |
    /// | 2    | `0b10`          | 16384 … 2^30 − 1     |
    /// | 3    | `0b11`          | big-integer mode      |
    pub fn decode_compact_u32(&mut self) -> Result<u32> {
        let first = self.read_byte()?;
        match first & 0b11 {
            0 => {
                // Single-byte mode: value in bits [7:2].
                Ok(u32::from(first >> 2))
            }
            1 => {
                // Two-byte mode: value in bits [15:2].
                let second = self.read_byte()?;
                let raw = u16::from_le_bytes([first, second]);
                Ok(u32::from(raw >> 2))
            }
            2 => {
                // Four-byte mode: value in bits [31:2].
                let b1 = self.read_byte()?;
                let b2 = self.read_byte()?;
                let b3 = self.read_byte()?;
                let raw = u32::from_le_bytes([first, b1, b2, b3]);
                Ok(raw >> 2)
            }
            3 => {
                // Big-integer mode: next `(first >> 2) + 4` bytes are the LE value.
                let byte_count = usize::from(first >> 2) + 4;
                if byte_count > 4 {
                    return Err(CodecError::decode(format!(
                        "compact big-integer too wide for u32: {byte_count} bytes"
                    )));
                }
                let bytes = self.read_bytes(byte_count)?;
                let mut buf = [0u8; 4];
                buf[..byte_count].copy_from_slice(bytes);
                Ok(u32::from_le_bytes(buf))
            }
            _ => unreachable!(),
        }
    }

    /// Decode a SCALE compact-encoded integer as a `u64`.
    ///
    /// Supports the big-integer mode up to 8 bytes, allowing values up to
    /// `u64::MAX`.
    pub fn decode_compact_u64(&mut self) -> Result<u64> {
        let first = self.read_byte()?;
        match first & 0b11 {
            0 => Ok(u64::from(first >> 2)),
            1 => {
                let second = self.read_byte()?;
                let raw = u16::from_le_bytes([first, second]);
                Ok(u64::from(raw >> 2))
            }
            2 => {
                let b1 = self.read_byte()?;
                let b2 = self.read_byte()?;
                let b3 = self.read_byte()?;
                let raw = u32::from_le_bytes([first, b1, b2, b3]);
                Ok(u64::from(raw >> 2))
            }
            3 => {
                let byte_count = usize::from(first >> 2) + 4;
                if byte_count > 8 {
                    return Err(CodecError::decode(format!(
                        "compact big-integer too wide for u64: {byte_count} bytes"
                    )));
                }
                let bytes = self.read_bytes(byte_count)?;
                let mut buf = [0u8; 8];
                buf[..byte_count].copy_from_slice(bytes);
                Ok(u64::from_le_bytes(buf))
            }
            _ => unreachable!(),
        }
    }

    /// Decode a length-prefixed byte vector (compact length + raw bytes).
    pub fn decode_bytes(&mut self) -> Result<Vec<u8>> {
        let len = self.decode_compact_u32()? as usize;
        let slice = self.read_bytes(len)?;
        Ok(slice.to_vec())
    }

    /// Decode a UTF-8 string (compact length + UTF-8 bytes).
    pub fn decode_string(&mut self) -> Result<String> {
        let bytes = self.decode_bytes()?;
        String::from_utf8(bytes).map_err(|e| {
            CodecError::invalid_type(format!("invalid UTF-8 string: {e}"))
        })
    }

    /// Decode a SCALE `Vec<T>` by first reading the compact element count,
    /// then calling `decode_fn` for each element.
    pub fn decode_vec<T, F>(&mut self, mut decode_fn: F) -> Result<Vec<T>>
    where
        F: FnMut(&mut Self) -> Result<T>,
    {
        let count = self.decode_compact_u32()? as usize;
        let mut out = Vec::with_capacity(count);
        for _ in 0..count {
            out.push(decode_fn(self)?);
        }
        Ok(out)
    }

    /// Decode a SCALE `Option<T>` (`0x00` = None, `0x01` prefix = Some).
    pub fn decode_option<T, F>(&mut self, decode_fn: F) -> Result<Option<T>>
    where
        F: FnOnce(&mut Self) -> Result<T>,
    {
        match self.read_byte()? {
            0x00 => Ok(None),
            0x01 => Ok(Some(decode_fn(self)?)),
            b => Err(CodecError::invalid_type(format!(
                "invalid Option prefix byte: 0x{b:02x}"
            ))),
        }
    }

    /// Decode a fixed-size byte array of length `N`.
    pub fn decode_fixed_array<const N: usize>(&mut self) -> Result<[u8; N]> {
        let bytes = self.read_bytes(N)?;
        let arr: [u8; N] = bytes.try_into().map_err(|_| {
            CodecError::decode(format!("fixed array slice of length {N} conversion failed"))
        })?;
        Ok(arr)
    }
}

// ---------------------------------------------------------------------------
// Encoder
// ---------------------------------------------------------------------------

/// A byte-buffer encoder that produces SCALE-encoded output.
///
/// The encoder appends to an internal `Vec<u8>`. Call [`ScaleEncoder::finish`]
/// to retrieve the completed bytes.
pub struct ScaleEncoder {
    buf: Vec<u8>,
}

impl ScaleEncoder {
    /// Create a new, empty encoder.
    pub fn new() -> Self {
        Self { buf: Vec::new() }
    }

    /// Create a new encoder with a pre-allocated capacity hint (bytes).
    pub fn with_capacity(cap: usize) -> Self {
        Self {
            buf: Vec::with_capacity(cap),
        }
    }

    /// Consume the encoder and return the encoded bytes.
    pub fn finish(self) -> Vec<u8> {
        self.buf
    }

    /// Return a reference to the bytes encoded so far.
    pub fn as_bytes(&self) -> &[u8] {
        &self.buf
    }

    // -----------------------------------------------------------------------
    // Primitive encoders
    // -----------------------------------------------------------------------

    /// Encode a `u8`.
    pub fn encode_u8(&mut self, v: u8) {
        self.buf.push(v);
    }

    /// Encode a `u16` as little-endian.
    pub fn encode_u16(&mut self, v: u16) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    /// Encode a `u32` as little-endian.
    pub fn encode_u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    /// Encode a `u64` as little-endian.
    pub fn encode_u64(&mut self, v: u64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    /// Encode a `u128` as little-endian.
    pub fn encode_u128(&mut self, v: u128) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    /// Encode a boolean (`false` → `0x00`, `true` → `0x01`).
    pub fn encode_bool(&mut self, v: bool) {
        self.buf.push(u8::from(v));
    }

    /// Encode a `u32` using SCALE compact encoding.
    ///
    /// | value range           | encoded bytes |
    /// |-----------------------|---------------|
    /// | 0 … 63               | 1             |
    /// | 64 … 16383           | 2             |
    /// | 16384 … 2^30 − 1     | 4             |
    /// | 2^30 … u32::MAX      | 5 (big-int mode, 1 extra byte) |
    pub fn encode_compact_u32(&mut self, v: u32) {
        if v <= 63 {
            self.buf.push((v as u8) << 2);
        } else if v <= 16_383 {
            let raw = ((v as u16) << 2) | 0b01;
            self.buf.extend_from_slice(&raw.to_le_bytes());
        } else if v <= 0x3FFF_FFFF {
            let raw = (v << 2) | 0b10;
            self.buf.extend_from_slice(&raw.to_le_bytes());
        } else {
            // Big-integer mode: 0b11 mode with byte count encoded in first byte.
            // For u32, we need at most 4 bytes.
            let bytes = v.to_le_bytes();
            // Find minimal byte count (strip trailing zeros).
            let mut len = 4usize;
            while len > 1 && bytes[len - 1] == 0 {
                len -= 1;
            }
            // first byte: (byte_count - 4) << 2 | 0b11, but byte_count >= 4 here.
            let extra = len.saturating_sub(4);
            self.buf.push(((extra as u8) << 2) | 0b11);
            self.buf.extend_from_slice(&bytes[..len]);
        }
    }

    /// Encode a `u64` using SCALE compact encoding.
    ///
    /// Supports the big-integer mode for values beyond `2^30 − 1`.
    pub fn encode_compact_u64(&mut self, v: u64) {
        if v <= 63 {
            self.buf.push((v as u8) << 2);
        } else if v <= 16_383 {
            let raw = ((v as u16) << 2) | 0b01;
            self.buf.extend_from_slice(&raw.to_le_bytes());
        } else if v <= 0x3FFF_FFFF {
            let raw = ((v as u32) << 2) | 0b10;
            self.buf.extend_from_slice(&raw.to_le_bytes());
        } else {
            // Big-integer mode.
            let bytes = v.to_le_bytes();
            let mut len = 8usize;
            while len > 1 && bytes[len - 1] == 0 {
                len -= 1;
            }
            // extra bytes beyond 4: (len - 4) << 2 | 0b11
            let extra = len.saturating_sub(4);
            self.buf.push(((extra as u8) << 2) | 0b11);
            self.buf.extend_from_slice(&bytes[..len]);
        }
    }

    /// Encode a raw byte slice with a compact length prefix.
    pub fn encode_bytes(&mut self, data: &[u8]) {
        self.encode_compact_u32(data.len() as u32);
        self.buf.extend_from_slice(data);
    }

    /// Encode a UTF-8 string with a compact length prefix.
    pub fn encode_string(&mut self, s: &str) {
        self.encode_bytes(s.as_bytes());
    }

    /// Encode a slice of `T` with a compact element count prefix, using
    /// `encode_fn` to encode each element.
    pub fn encode_vec<T, F>(&mut self, items: &[T], mut encode_fn: F)
    where
        F: FnMut(&mut Self, &T),
    {
        self.encode_compact_u32(items.len() as u32);
        for item in items {
            encode_fn(self, item);
        }
    }

    /// Encode an `Option<T>` (`None` → `0x00`, `Some(v)` → `0x01` + encoded value).
    pub fn encode_option<T, F>(&mut self, opt: Option<&T>, encode_fn: F)
    where
        F: FnOnce(&mut Self, &T),
    {
        match opt {
            None => self.buf.push(0x00),
            Some(v) => {
                self.buf.push(0x01);
                encode_fn(self, v);
            }
        }
    }
}

impl Default for ScaleEncoder {
    fn default() -> Self {
        Self::new()
    }
}
