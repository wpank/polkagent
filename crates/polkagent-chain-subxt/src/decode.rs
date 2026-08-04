//! SCALE decoding helpers using `polkagent-codec`.
//!
//! This module bridges between raw hex-encoded RPC responses and the typed
//! domain objects expected by the [`ChainClient`] trait.
//!
//! [`ChainClient`]: polkagent_chain_trait::ChainClient

use polkagent_chain_trait::{DecodedCall, MetadataDigest};
use polkagent_codec::{
    decode_call_with_metadata, parse_metadata, CodecError, DecodedField, FieldValue,
    RuntimeMetadata,
};

use crate::error::SubxtError;

// ---------------------------------------------------------------------------
// Hex utilities
// ---------------------------------------------------------------------------

/// Decode a hex string (with optional `0x` prefix) into bytes.
pub fn hex_to_bytes(hex: &str) -> Result<Vec<u8>, SubxtError> {
    let stripped = hex.strip_prefix("0x").unwrap_or(hex);
    if stripped.len() % 2 != 0 {
        return Err(SubxtError::HexDecode {
            message: format!("odd-length hex string ({})", stripped.len()),
        });
    }
    let mut out = Vec::with_capacity(stripped.len() / 2);
    for i in (0..stripped.len()).step_by(2) {
        let byte_str = &stripped[i..i + 2];
        let byte = u8::from_str_radix(byte_str, 16).map_err(|_| SubxtError::HexDecode {
            message: format!("invalid hex byte: {byte_str}"),
        })?;
        out.push(byte);
    }
    Ok(out)
}

/// Encode bytes as a `0x`-prefixed hex string.
pub fn bytes_to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(2 + bytes.len() * 2);
    s.push_str("0x");
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

// ---------------------------------------------------------------------------
// Metadata parsing
// ---------------------------------------------------------------------------

/// Parse raw SCALE-encoded metadata bytes into a [`RuntimeMetadata`] struct.
pub fn parse_runtime_metadata(metadata_bytes: &[u8]) -> Result<RuntimeMetadata, SubxtError> {
    parse_metadata(metadata_bytes).map_err(|e| SubxtError::Metadata {
        message: format!("failed to parse runtime metadata: {e}"),
    })
}

/// Compute the BLAKE3 digest of metadata bytes and return it as a hex string.
pub fn compute_metadata_digest(metadata_bytes: &[u8]) -> MetadataDigest {
    let hash = blake3::hash(metadata_bytes);
    MetadataDigest(format!("0x{hash}"))
}

// ---------------------------------------------------------------------------
// Call decoding
// ---------------------------------------------------------------------------

/// Decode SCALE-encoded call bytes using the provided metadata.
///
/// This function does not make any RPC calls -- all information required for
/// decoding comes from the metadata bytes.
pub fn decode_call_bytes(
    call_bytes: &[u8],
    metadata_bytes: &[u8],
    metadata_digest: &MetadataDigest,
) -> Result<DecodedCall, SubxtError> {
    let runtime_metadata = parse_runtime_metadata(metadata_bytes)?;
    decode_call_with_runtime_metadata(call_bytes, &runtime_metadata, metadata_digest)
}

/// Decode SCALE-encoded call bytes using a pre-parsed [`RuntimeMetadata`].
pub fn decode_call_with_runtime_metadata(
    call_bytes: &[u8],
    metadata: &RuntimeMetadata,
    metadata_digest: &MetadataDigest,
) -> Result<DecodedCall, SubxtError> {
    if call_bytes.len() < 2 {
        return Err(SubxtError::ScaleDecode {
            message: "call bytes too short: need at least 2 bytes for pallet and call indices"
                .into(),
        });
    }

    let decoded = decode_call_with_metadata(call_bytes, metadata).map_err(|e: CodecError| {
        SubxtError::ScaleDecode {
            message: format!("failed to decode call: {e}"),
        }
    })?;

    let pallet = decoded
        .pallet_name
        .unwrap_or_else(|| format!("Pallet#{}", decoded.pallet_index));
    let call_name = decoded
        .call_name
        .unwrap_or_else(|| format!("call#{}", decoded.call_index));

    let arguments_json = fields_to_json(&decoded.args);

    Ok(DecodedCall {
        pallet,
        call_name,
        arguments_json,
        metadata_digest: metadata_digest.clone(),
    })
}

/// Convert decoded fields to a JSON string.
fn fields_to_json(fields: &[DecodedField]) -> String {
    let obj = fields_to_json_value(fields);
    serde_json::to_string(&obj).unwrap_or_else(|_| "{}".to_string())
}

/// Convert decoded fields to a JSON value.
fn fields_to_json_value(fields: &[DecodedField]) -> serde_json::Value {
    if fields.iter().all(|f| f.name.is_some()) {
        // Named fields -> JSON object.
        let map: serde_json::Map<String, serde_json::Value> = fields
            .iter()
            .map(|f| {
                let key = f.name.clone().unwrap_or_default();
                let value = field_value_to_json(&f.value);
                (key, value)
            })
            .collect();
        serde_json::Value::Object(map)
    } else {
        // Unnamed fields -> JSON array.
        let arr: Vec<serde_json::Value> = fields
            .iter()
            .map(|f| field_value_to_json(&f.value))
            .collect();
        serde_json::Value::Array(arr)
    }
}

/// Convert a single [`FieldValue`] to a JSON value.
fn field_value_to_json(value: &FieldValue) -> serde_json::Value {
    match value {
        FieldValue::U8(v) => serde_json::json!(*v),
        FieldValue::U16(v) => serde_json::json!(*v),
        FieldValue::U32(v) => serde_json::json!(*v),
        FieldValue::U64(v) => serde_json::json!(*v),
        FieldValue::U128(v) => serde_json::json!(v.to_string()),
        FieldValue::Bool(v) => serde_json::json!(*v),
        FieldValue::String(s) => serde_json::json!(s),
        FieldValue::Bytes(b) => serde_json::json!(bytes_to_hex(b)),
        FieldValue::AccountId(id) => serde_json::json!(bytes_to_hex(id)),
        FieldValue::Compact(v) => serde_json::json!(v.to_string()),
        FieldValue::Sequence(items) => {
            let arr: Vec<serde_json::Value> = items.iter().map(field_value_to_json).collect();
            serde_json::Value::Array(arr)
        }
        FieldValue::Composite(fields) => fields_to_json_value(fields),
        FieldValue::Variant {
            index: _,
            name,
            fields,
        } => {
            let variant_name = name.clone().unwrap_or_else(|| "unknown".to_string());
            if fields.is_empty() {
                serde_json::json!(variant_name)
            } else {
                serde_json::json!({
                    "variant": variant_name,
                    "fields": fields_to_json_value(fields),
                })
            }
        }
    }
}

/// Extract a block number from a hex-encoded header `number` field.
///
/// Substrate returns block numbers as hex strings like `"0x1234"`.
pub fn parse_block_number_hex(hex: &str) -> Result<u64, SubxtError> {
    let stripped = hex.strip_prefix("0x").unwrap_or(hex);
    u64::from_str_radix(stripped, 16).map_err(|_| SubxtError::HexDecode {
        message: format!("invalid block number hex: {hex}"),
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_to_bytes_with_prefix() {
        let bytes = hex_to_bytes("0xdeadbeef").expect("decode");
        assert_eq!(bytes, vec![0xDE, 0xAD, 0xBE, 0xEF]);
    }

    #[test]
    fn hex_to_bytes_without_prefix() {
        let bytes = hex_to_bytes("cafebabe").expect("decode");
        assert_eq!(bytes, vec![0xCA, 0xFE, 0xBA, 0xBE]);
    }

    #[test]
    fn hex_to_bytes_empty() {
        let bytes = hex_to_bytes("0x").expect("decode");
        assert!(bytes.is_empty());
    }

    #[test]
    fn hex_to_bytes_odd_length_errors() {
        let result = hex_to_bytes("0xabc");
        assert!(result.is_err());
    }

    #[test]
    fn hex_to_bytes_invalid_char_errors() {
        let result = hex_to_bytes("0xGG");
        assert!(result.is_err());
    }

    #[test]
    fn bytes_to_hex_round_trip() {
        let original = vec![0xDE, 0xAD, 0xBE, 0xEF];
        let hex = bytes_to_hex(&original);
        assert_eq!(hex, "0xdeadbeef");
        let decoded = hex_to_bytes(&hex).expect("decode");
        assert_eq!(decoded, original);
    }

    #[test]
    fn bytes_to_hex_empty() {
        assert_eq!(bytes_to_hex(&[]), "0x");
    }

    #[test]
    fn compute_metadata_digest_deterministic() {
        let data = b"test metadata bytes";
        let d1 = compute_metadata_digest(data);
        let d2 = compute_metadata_digest(data);
        assert_eq!(d1.0, d2.0);
        assert!(d1.0.starts_with("0x"));
    }

    #[test]
    fn compute_metadata_digest_differs_for_different_input() {
        let d1 = compute_metadata_digest(b"aaa");
        let d2 = compute_metadata_digest(b"bbb");
        assert_ne!(d1.0, d2.0);
    }

    #[test]
    fn parse_block_number_hex_valid() {
        assert_eq!(parse_block_number_hex("0x0").expect("ok"), 0);
        assert_eq!(parse_block_number_hex("0xff").expect("ok"), 255);
        assert_eq!(parse_block_number_hex("0x100").expect("ok"), 256);
        assert_eq!(parse_block_number_hex("0xf4240").expect("ok"), 1_000_000);
    }

    #[test]
    fn parse_block_number_hex_without_prefix() {
        assert_eq!(parse_block_number_hex("ff").expect("ok"), 255);
    }

    #[test]
    fn parse_block_number_hex_invalid() {
        assert!(parse_block_number_hex("0xZZZ").is_err());
    }

    #[test]
    fn field_value_to_json_primitives() {
        assert_eq!(
            field_value_to_json(&FieldValue::U8(42)),
            serde_json::json!(42)
        );
        assert_eq!(
            field_value_to_json(&FieldValue::Bool(true)),
            serde_json::json!(true)
        );
        assert_eq!(
            field_value_to_json(&FieldValue::String("hello".into())),
            serde_json::json!("hello")
        );
    }

    #[test]
    fn field_value_to_json_u128_as_string() {
        let val = field_value_to_json(&FieldValue::U128(u128::MAX));
        assert!(val.is_string());
    }

    #[test]
    fn field_value_to_json_bytes() {
        let val = field_value_to_json(&FieldValue::Bytes(vec![0xDE, 0xAD]));
        assert_eq!(val, serde_json::json!("0xdead"));
    }

    #[test]
    fn field_value_to_json_account_id() {
        let id = [0x01u8; 32];
        let val = field_value_to_json(&FieldValue::AccountId(id));
        assert!(val.as_str().expect("string").starts_with("0x"));
    }

    #[test]
    fn fields_to_json_named_fields() {
        let fields = vec![
            DecodedField::named("amount", FieldValue::U64(1000)),
            DecodedField::named("dest", FieldValue::String("Alice".into())),
        ];
        let json_str = fields_to_json(&fields);
        let parsed: serde_json::Value = serde_json::from_str(&json_str).expect("parse");
        assert_eq!(parsed["amount"], serde_json::json!(1000));
        assert_eq!(parsed["dest"], serde_json::json!("Alice"));
    }

    #[test]
    fn fields_to_json_unnamed_fields() {
        let fields = vec![
            DecodedField::unnamed(FieldValue::U8(1)),
            DecodedField::unnamed(FieldValue::U8(2)),
        ];
        let json_str = fields_to_json(&fields);
        let parsed: serde_json::Value = serde_json::from_str(&json_str).expect("parse");
        assert!(parsed.is_array());
        assert_eq!(parsed[0], serde_json::json!(1));
        assert_eq!(parsed[1], serde_json::json!(2));
    }

    #[test]
    fn decode_call_bytes_too_short() {
        let digest = MetadataDigest("0xtest".into());
        let result = decode_call_bytes(&[0x01], &[], &digest);
        assert!(result.is_err());
    }

    #[test]
    fn field_value_to_json_compact() {
        let val = field_value_to_json(&FieldValue::Compact(999));
        assert_eq!(val, serde_json::json!("999"));
    }

    #[test]
    fn field_value_to_json_variant_no_fields() {
        let val = field_value_to_json(&FieldValue::Variant {
            index: 0,
            name: Some("None".into()),
            fields: vec![],
        });
        assert_eq!(val, serde_json::json!("None"));
    }

    #[test]
    fn field_value_to_json_variant_with_fields() {
        let val = field_value_to_json(&FieldValue::Variant {
            index: 1,
            name: Some("Some".into()),
            fields: vec![DecodedField::unnamed(FieldValue::U32(42))],
        });
        let obj = val.as_object().expect("object");
        assert_eq!(obj["variant"], serde_json::json!("Some"));
    }

    #[test]
    fn field_value_to_json_sequence() {
        let val = field_value_to_json(&FieldValue::Sequence(vec![
            FieldValue::U8(1),
            FieldValue::U8(2),
            FieldValue::U8(3),
        ]));
        assert!(val.is_array());
        assert_eq!(val.as_array().expect("arr").len(), 3);
    }
}
