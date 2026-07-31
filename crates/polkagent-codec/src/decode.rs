//! Higher-level extrinsic decoding built on top of the core SCALE primitives.
//!
//! [`decode_extrinsic`] performs a minimal structural decode (pallet/call
//! indices + raw remaining args). [`decode_call_with_metadata`] uses the full
//! type registry to recursively decode each argument into typed [`FieldValue`]s.

use std::fmt;

use crate::{
    error::{CodecError, Result},
    metadata::{PrimitiveType, RuntimeMetadata, TypeDef},
    scale::ScaleDecoder,
};

// ---------------------------------------------------------------------------
// FieldValue
// ---------------------------------------------------------------------------

/// A fully decoded value of any SCALE type.
#[derive(Debug, Clone, PartialEq)]
pub enum FieldValue {
    /// `u8` scalar.
    U8(u8),
    /// `u16` scalar.
    U16(u16),
    /// `u32` scalar.
    U32(u32),
    /// `u64` scalar.
    U64(u64),
    /// `u128` scalar.
    U128(u128),
    /// Boolean.
    Bool(bool),
    /// UTF-8 string.
    String(String),
    /// Raw byte sequence.
    Bytes(Vec<u8>),
    /// 32-byte account ID (SS58 address pre-image).
    AccountId([u8; 32]),
    /// Compact-encoded integer (stored as `u128` for losslessness).
    Compact(u128),
    /// A variable-length sequence of homogeneous values.
    Sequence(Vec<FieldValue>),
    /// A composite (struct) value with named or unnamed fields.
    Composite(Vec<DecodedField>),
    /// An enum variant.
    Variant {
        /// Variant discriminant index.
        index: u8,
        /// Optional human-readable variant name (requires metadata).
        name: Option<String>,
        /// Variant payload fields.
        fields: Vec<DecodedField>,
    },
}

impl fmt::Display for FieldValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::U8(v) => write!(f, "{v}"),
            Self::U16(v) => write!(f, "{v}"),
            Self::U32(v) => write!(f, "{v}"),
            Self::U64(v) => write!(f, "{v}"),
            Self::U128(v) => write!(f, "{v}"),
            Self::Bool(v) => write!(f, "{v}"),
            Self::String(s) => write!(f, "\"{s}\""),
            Self::Bytes(b) => write!(f, "0x{}", hex_encode(b)),
            Self::AccountId(id) => write!(f, "0x{}", hex_encode(id)),
            Self::Compact(v) => write!(f, "compact({v})"),
            Self::Sequence(items) => {
                write!(f, "[")?;
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{item}")?;
                }
                write!(f, "]")
            }
            Self::Composite(fields) => {
                write!(f, "{{")?;
                for (i, field) in fields.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    if let Some(name) = &field.name {
                        write!(f, "{name}: ")?;
                    }
                    write!(f, "{}", field.value)?;
                }
                write!(f, "}}")
            }
            Self::Variant { index, name, fields } => {
                if let Some(n) = name {
                    write!(f, "{n}")?;
                } else {
                    write!(f, "#{index}")?;
                }
                if !fields.is_empty() {
                    write!(f, "{{")?;
                    for (i, field) in fields.iter().enumerate() {
                        if i > 0 {
                            write!(f, ", ")?;
                        }
                        write!(f, "{}", field.value)?;
                    }
                    write!(f, "}}")?;
                }
                Ok(())
            }
        }
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

// ---------------------------------------------------------------------------
// DecodedField
// ---------------------------------------------------------------------------

/// A single decoded field, optionally named.
#[derive(Debug, Clone, PartialEq)]
pub struct DecodedField {
    /// Optional field name (from metadata). `None` for unnamed/positional fields.
    pub name: Option<String>,
    /// The decoded value.
    pub value: FieldValue,
}

impl DecodedField {
    /// Create a new named decoded field.
    pub fn named(name: impl Into<String>, value: FieldValue) -> Self {
        Self {
            name: Some(name.into()),
            value,
        }
    }

    /// Create a new unnamed decoded field.
    pub fn unnamed(value: FieldValue) -> Self {
        Self { name: None, value }
    }
}

// ---------------------------------------------------------------------------
// DecodedExtrinsic
// ---------------------------------------------------------------------------

/// The result of decoding a Substrate extrinsic.
#[derive(Debug, Clone)]
pub struct DecodedExtrinsic {
    /// Pallet index from the first byte of the call data.
    pub pallet_index: u8,
    /// Call index from the second byte of the call data.
    pub call_index: u8,
    /// Human-readable pallet name (populated when metadata is available).
    pub pallet_name: Option<String>,
    /// Human-readable call name (populated when metadata is available).
    pub call_name: Option<String>,
    /// Decoded call arguments. Raw bytes are wrapped in [`FieldValue::Bytes`]
    /// when no metadata is available for typed decoding.
    pub args: Vec<DecodedField>,
}

impl DecodedExtrinsic {
    /// Return the raw bytes of all args concatenated, if every arg is
    /// [`FieldValue::Bytes`].
    pub fn raw_args_bytes(&self) -> Option<Vec<u8>> {
        let mut out = Vec::new();
        for field in &self.args {
            if let FieldValue::Bytes(b) = &field.value {
                out.extend_from_slice(b);
            } else {
                return None;
            }
        }
        Some(out)
    }
}

// ---------------------------------------------------------------------------
// decode_extrinsic — minimal structural decode
// ---------------------------------------------------------------------------

/// Decode an extrinsic from raw bytes without metadata.
///
/// This performs a minimal structural decode:
///
/// 1. Read the outer compact length prefix (as emitted by `encode_extrinsic`).
/// 2. Read the version/signed byte.
/// 3. If signed, skip the 32-byte sender, 1-byte signature type, and 64-byte
///    signature, plus the `extra` signed extensions.
/// 4. Read `pallet_index` and `call_index`.
/// 5. Treat the remaining bytes as a single raw [`FieldValue::Bytes`] arg.
pub fn decode_extrinsic(bytes: &[u8]) -> Result<DecodedExtrinsic> {
    let mut dec = ScaleDecoder::new(bytes);

    // Outer compact length prefix.
    let _len = dec.decode_compact_u32()?;

    // Version byte: low 7 bits = version, high bit = signed flag.
    let version_byte = dec.decode_u8()?;
    let is_signed = (version_byte & 0x80) != 0;

    if is_signed {
        // sender: MultiAddress (enum variant byte + 32-byte AccountId).
        let _addr_variant = dec.decode_u8()?;
        let _sender: [u8; 32] = dec.decode_fixed_array::<32>()?;
        // signature: MultiSignature (enum variant byte + payload).
        let sig_variant = dec.decode_u8()?;
        let _sig_bytes = match sig_variant {
            0 | 1 => dec.decode_fixed_array::<64>()?.to_vec(), // Ed25519 / Sr25519
            2 => dec.decode_fixed_array::<65>()?.to_vec(),     // Ecdsa
            v => {
                return Err(CodecError::invalid_type(format!(
                    "unknown signature variant: {v}"
                )))
            }
        };
        // signed extensions (era, nonce, tip, …) — skip remaining bytes
        // by reading raw until we hit the call data. We use a heuristic:
        // the era is either a single 0x00 (immortal) or 2 bytes (mortal).
        let era_byte = dec.decode_u8()?;
        if era_byte != 0x00 {
            // Mortal era: one more byte.
            let _era_high = dec.decode_u8()?;
        }
        // nonce: compact u64
        let _nonce = dec.decode_compact_u64()?;
        // tip: compact u128
        let _tip = decode_compact_u128_raw(&mut dec)?;
    }

    // Call data: pallet_index + call_index.
    let pallet_index = dec.decode_u8()?;
    let call_index = dec.decode_u8()?;

    // Remaining bytes are the raw call arguments.
    let remaining = dec.remaining_bytes().to_vec();
    let args = if remaining.is_empty() {
        vec![]
    } else {
        vec![DecodedField::unnamed(FieldValue::Bytes(remaining))]
    };

    Ok(DecodedExtrinsic {
        pallet_index,
        call_index,
        pallet_name: None,
        call_name: None,
        args,
    })
}

// ---------------------------------------------------------------------------
// decode_call_with_metadata — full recursive typed decode
// ---------------------------------------------------------------------------

/// Decode a bare call (starting from `pallet_index` byte, no outer length
/// prefix) using the type registry in `metadata`.
///
/// The `bytes` slice must begin with `[pallet_index, call_index, args...]`.
pub fn decode_call_with_metadata(
    bytes: &[u8],
    metadata: &RuntimeMetadata,
) -> Result<DecodedExtrinsic> {
    let mut dec = ScaleDecoder::new(bytes);

    let pallet_index = dec.decode_u8()?;
    let call_index = dec.decode_u8()?;

    let pallet = metadata.pallet_by_index(pallet_index);
    let pallet_name = pallet.map(|p| p.name.clone());

    let call_meta = metadata.call_by_indices(pallet_index, call_index);
    let call_name = call_meta.map(|c| c.name.clone());

    let args = if let Some(call) = call_meta {
        let mut fields = Vec::with_capacity(call.fields.len());
        for field_meta in &call.fields {
            let value = decode_value(&mut dec, field_meta.type_id, metadata)?;
            fields.push(DecodedField {
                name: field_meta.name.clone(),
                value,
            });
        }
        fields
    } else {
        // No metadata for this call — return remaining bytes raw.
        let remaining = dec.remaining_bytes().to_vec();
        if remaining.is_empty() {
            vec![]
        } else {
            vec![DecodedField::unnamed(FieldValue::Bytes(remaining))]
        }
    };

    Ok(DecodedExtrinsic {
        pallet_index,
        call_index,
        pallet_name,
        call_name,
        args,
    })
}

// ---------------------------------------------------------------------------
// Recursive type decoder
// ---------------------------------------------------------------------------

/// Decode a single value of the type identified by `type_id` from the decoder.
pub fn decode_value(
    dec: &mut ScaleDecoder<'_>,
    type_id: u32,
    metadata: &RuntimeMetadata,
) -> Result<FieldValue> {
    let type_def = metadata.types.get(type_id).ok_or_else(|| {
        CodecError::decode(format!("unknown type id: {type_id}"))
    })?;

    match type_def.clone() {
        TypeDef::Primitive(prim) => decode_primitive_value(dec, &prim),
        TypeDef::Compact(inner_id) => {
            // Compact encoding: value is a compact-encoded integer.
            // The underlying type tells us the max width.
            let v = decode_compact_u128_raw(dec)?;
            let _ = inner_id;
            Ok(FieldValue::Compact(v))
        }
        TypeDef::Sequence(elem_id) => {
            let count = dec.decode_compact_u32()? as usize;
            // Special-case: sequence of u8 is Bytes.
            if is_u8_type(elem_id, metadata) {
                let bytes = read_n_bytes(dec, count)?;
                Ok(FieldValue::Bytes(bytes))
            } else {
                let mut items = Vec::with_capacity(count);
                for _ in 0..count {
                    items.push(decode_value(dec, elem_id, metadata)?);
                }
                Ok(FieldValue::Sequence(items))
            }
        }
        TypeDef::Array { len, type_id: elem_id } => {
            let count = len as usize;
            // Special-case: [u8; 32] is AccountId.
            if len == 32 && is_u8_type(elem_id, metadata) {
                let arr: [u8; 32] = dec.decode_fixed_array::<32>()?;
                Ok(FieldValue::AccountId(arr))
            } else if is_u8_type(elem_id, metadata) {
                let bytes = read_n_bytes(dec, count)?;
                Ok(FieldValue::Bytes(bytes))
            } else {
                let mut items = Vec::with_capacity(count);
                for _ in 0..count {
                    items.push(decode_value(dec, elem_id, metadata)?);
                }
                Ok(FieldValue::Sequence(items))
            }
        }
        TypeDef::Tuple(ids) => {
            let mut fields = Vec::with_capacity(ids.len());
            for id in &ids {
                let val = decode_value(dec, *id, metadata)?;
                fields.push(DecodedField::unnamed(val));
            }
            Ok(FieldValue::Composite(fields))
        }
        TypeDef::Composite { fields: field_metas } => {
            // Check if this is a Vec<u8> / BoundedVec<u8> composite.
            // Heuristic: single unnamed field of Sequence(u8).
            if field_metas.len() == 1 && field_metas[0].name.is_none() {
                if let Some(TypeDef::Sequence(elem_id)) =
                    metadata.types.get(field_metas[0].type_id).cloned()
                {
                    if is_u8_type(elem_id, metadata) {
                        return Ok(FieldValue::Bytes(dec.decode_bytes()?));
                    }
                }
            }
            let mut fields = Vec::with_capacity(field_metas.len());
            for fm in &field_metas {
                let val = decode_value(dec, fm.type_id, metadata)?;
                fields.push(DecodedField {
                    name: fm.name.clone(),
                    value: val,
                });
            }
            Ok(FieldValue::Composite(fields))
        }
        TypeDef::Variant { variants } => {
            let index = dec.decode_u8()?;
            let variant_def = variants.iter().find(|v| v.index == index);
            let name = variant_def.map(|v| v.name.clone());
            let field_metas = variant_def
                .map(|v| v.fields.clone())
                .unwrap_or_default();
            let mut fields = Vec::with_capacity(field_metas.len());
            for fm in &field_metas {
                let val = decode_value(dec, fm.type_id, metadata)?;
                fields.push(DecodedField {
                    name: fm.name.clone(),
                    value: val,
                });
            }
            Ok(FieldValue::Variant {
                index,
                name,
                fields,
            })
        }
        TypeDef::Opaque => {
            // Cannot decode; return remaining bytes.
            let remaining = dec.remaining_bytes().to_vec();
            Ok(FieldValue::Bytes(remaining))
        }
    }
}

fn decode_primitive_value(dec: &mut ScaleDecoder<'_>, prim: &PrimitiveType) -> Result<FieldValue> {
    match prim {
        PrimitiveType::Bool => Ok(FieldValue::Bool(dec.decode_bool()?)),
        // Signed integers are stored identically to their unsigned counterparts.
        PrimitiveType::U8 | PrimitiveType::I8 => Ok(FieldValue::U8(dec.decode_u8()?)),
        PrimitiveType::U16 | PrimitiveType::I16 => Ok(FieldValue::U16(dec.decode_u16()?)),
        PrimitiveType::U32 | PrimitiveType::I32 => Ok(FieldValue::U32(dec.decode_u32()?)),
        PrimitiveType::U64 | PrimitiveType::I64 => Ok(FieldValue::U64(dec.decode_u64()?)),
        PrimitiveType::U128 | PrimitiveType::I128 => Ok(FieldValue::U128(dec.decode_u128()?)),
        PrimitiveType::Str => Ok(FieldValue::String(dec.decode_string()?)),
        PrimitiveType::Bytes => Ok(FieldValue::Bytes(dec.decode_bytes()?)),
        PrimitiveType::Char => {
            let code = dec.decode_u32()?;
            char::from_u32(code).map(|c| FieldValue::String(c.to_string())).ok_or_else(|| {
                CodecError::invalid_type(format!("invalid unicode scalar: {code}"))
            })
        }
        PrimitiveType::U256 => {
            let bytes = read_n_bytes(dec, 32)?;
            Ok(FieldValue::Bytes(bytes))
        }
    }
}

fn is_u8_type(type_id: u32, metadata: &RuntimeMetadata) -> bool {
    matches!(
        metadata.types.get(type_id),
        Some(TypeDef::Primitive(PrimitiveType::U8))
    )
}

// ---------------------------------------------------------------------------
// Module-local helpers wrapping ScaleDecoder
// ---------------------------------------------------------------------------

/// Decode a compact-encoded `u128` from `dec` (widened from compact-u64).
fn decode_compact_u128_raw(dec: &mut ScaleDecoder<'_>) -> Result<u128> {
    let v = dec.decode_compact_u64()?;
    Ok(u128::from(v))
}

/// Read exactly `n` bytes from `dec` into a `Vec<u8>`.
fn read_n_bytes(dec: &mut ScaleDecoder<'_>, n: usize) -> Result<Vec<u8>> {
    if dec.remaining() < n {
        return Err(CodecError::unexpected_eof(dec.position()));
    }
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        out.push(dec.decode_u8()?);
    }
    Ok(out)
}
