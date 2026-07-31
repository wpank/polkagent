//! Helpers for well-known Polkadot call types.
//!
//! These functions recognise the standard Substrate `Utility.batch`,
//! `Utility.batchAll`, `Proxy.proxy`, and `Balances.transferKeepAlive` /
//! `Balances.transfer` calls and extract useful information from them without
//! requiring full metadata.

use crate::{
    decode::{DecodedExtrinsic, DecodedField, FieldValue},
    error::{CodecError, Result},
    scale::ScaleDecoder,
};

// ---------------------------------------------------------------------------
// Pallet / call index constants
// (These are the canonical values on Polkadot mainnet; adjust as needed for
//  other runtimes.)
// ---------------------------------------------------------------------------

/// Well-known pallet indices on Polkadot mainnet.
pub mod pallet_index {
    /// `Utility` pallet index.
    pub const UTILITY: u8 = 24;
    /// `Proxy` pallet index.
    pub const PROXY: u8 = 29;
    /// `Balances` pallet index.
    pub const BALANCES: u8 = 5;
}

/// Well-known call indices within their respective pallets.
pub mod call_index {
    /// `Utility.batch`
    pub const UTILITY_BATCH: u8 = 0;
    /// `Utility.batchAll`
    pub const UTILITY_BATCH_ALL: u8 = 2;
    /// `Utility.forceBatch`
    pub const UTILITY_FORCE_BATCH: u8 = 4;
    /// `Proxy.proxy`
    pub const PROXY_PROXY: u8 = 0;
    /// `Balances.transfer`
    pub const BALANCES_TRANSFER: u8 = 0;
    /// `Balances.transferKeepAlive`
    pub const BALANCES_TRANSFER_KEEP_ALIVE: u8 = 3;
    /// `Balances.transferAll`
    pub const BALANCES_TRANSFER_ALL: u8 = 4;
}

// ---------------------------------------------------------------------------
// Predicate helpers
// ---------------------------------------------------------------------------

/// Return `true` if `ext` is a `Utility.batch`, `batchAll`, or `forceBatch`.
pub fn is_batch_call(ext: &DecodedExtrinsic) -> bool {
    ext.pallet_index == pallet_index::UTILITY
        && matches!(
            ext.call_index,
            call_index::UTILITY_BATCH
                | call_index::UTILITY_BATCH_ALL
                | call_index::UTILITY_FORCE_BATCH
        )
}

/// Return `true` if `ext` is a `Proxy.proxy` call.
pub fn is_proxy_call(ext: &DecodedExtrinsic) -> bool {
    ext.pallet_index == pallet_index::PROXY && ext.call_index == call_index::PROXY_PROXY
}

/// Return `true` if `ext` is any Balances transfer variant.
pub fn is_transfer_call(ext: &DecodedExtrinsic) -> bool {
    ext.pallet_index == pallet_index::BALANCES
        && matches!(
            ext.call_index,
            call_index::BALANCES_TRANSFER
                | call_index::BALANCES_TRANSFER_KEEP_ALIVE
                | call_index::BALANCES_TRANSFER_ALL
        )
}

// ---------------------------------------------------------------------------
// decode_batch_call
// ---------------------------------------------------------------------------

/// Recursively decode the inner calls of a `Utility.batch` / `batchAll` /
/// `forceBatch` extrinsic.
///
/// The encoding of the inner call list is:
/// ```text
/// [compact count] [inner_call_0 ...] [inner_call_1 ...] ...
/// ```
/// where each `inner_call` is a bare call byte sequence (no outer length
/// prefix) prefixed by a compact byte-length so the decoder can advance.
///
/// # Errors
///
/// Returns [`CodecError::InvalidType`] if `ext` is not a batch call.
pub fn decode_batch_call(ext: &DecodedExtrinsic) -> Result<Vec<DecodedExtrinsic>> {
    if !is_batch_call(ext) {
        return Err(CodecError::invalid_type(format!(
            "expected a Utility batch call, got pallet={} call={}",
            ext.pallet_index, ext.call_index
        )));
    }

    let raw = extract_raw_args(ext)?;
    let mut dec = ScaleDecoder::new(&raw);

    // Number of inner calls (compact u32).
    let count = dec.decode_compact_u32()? as usize;
    let mut calls = Vec::with_capacity(count);

    for _ in 0..count {
        // Each inner call is length-prefixed (compact bytes).
        let call_bytes = dec.decode_bytes()?;
        let inner = decode_bare_call(&call_bytes)?;
        calls.push(inner);
    }

    Ok(calls)
}

// ---------------------------------------------------------------------------
// decode_proxy_call
// ---------------------------------------------------------------------------

/// Extract the inner call from a `Proxy.proxy` extrinsic.
///
/// The proxy call encoding (simplified) is:
/// ```text
/// [real: MultiAddress = 1 byte + 32 bytes]
/// [force_proxy_type: Option<u8>]
/// [call: Vec<u8> (length-prefixed bare call)]
/// ```
///
/// # Errors
///
/// Returns [`CodecError::InvalidType`] if `ext` is not a proxy call.
pub fn decode_proxy_call(ext: &DecodedExtrinsic) -> Result<DecodedExtrinsic> {
    if !is_proxy_call(ext) {
        return Err(CodecError::invalid_type(format!(
            "expected a Proxy.proxy call, got pallet={} call={}",
            ext.pallet_index, ext.call_index
        )));
    }

    let raw = extract_raw_args(ext)?;
    let mut dec = ScaleDecoder::new(&raw);

    // `real`: MultiAddress — variant byte + payload.
    // Id variant (0x00) = 32-byte AccountId.
    let addr_variant = dec.decode_u8()?;
    match addr_variant {
        0x00 => {
            let _real: [u8; 32] = dec.decode_fixed_array::<32>()?;
        }
        0x01 => {
            // Index: compact u32.
            let _idx = dec.decode_compact_u32()?;
        }
        0x02 => {
            // Raw: Vec<u8>.
            let _raw = dec.decode_bytes()?;
        }
        0x03 => {
            // Address32.
            let _a: [u8; 32] = dec.decode_fixed_array::<32>()?;
        }
        0x04 => {
            // Address20.
            let _a: [u8; 20] = dec.decode_fixed_array::<20>()?;
        }
        v => {
            return Err(CodecError::invalid_type(format!(
                "unknown MultiAddress variant: {v}"
            )));
        }
    }

    // `forceProxyType`: Option<u8>
    let _force_proxy_type: Option<u8> = dec.decode_option(ScaleDecoder::decode_u8)?;

    // `call`: length-prefixed bare call bytes.
    let call_bytes = dec.decode_bytes()?;
    decode_bare_call(&call_bytes)
}

// ---------------------------------------------------------------------------
// extract_transfer_amount
// ---------------------------------------------------------------------------

/// Extract the transfer amount from a `Balances.transfer*` extrinsic.
///
/// The transfer amount is a compact-encoded `u128` value that appears
/// immediately after the destination address in the call encoding:
/// ```text
/// [dest: MultiAddress] [value: compact u128]
/// ```
///
/// Returns `None` if `ext` is not a transfer call or decoding fails.
pub fn extract_transfer_amount(ext: &DecodedExtrinsic) -> Option<u128> {
    if !is_transfer_call(ext) {
        // Check named args first.
        return extract_amount_from_named_args(ext);
    }

    // Try named field first.
    if let Some(v) = extract_amount_from_named_args(ext) {
        return Some(v);
    }

    // Fall back to raw bytes decode.
    let raw = extract_raw_args(ext).ok()?;
    let mut dec = ScaleDecoder::new(&raw);

    // Skip destination MultiAddress.
    let addr_variant = dec.decode_u8().ok()?;
    match addr_variant {
        0x00 | 0x03 => {
            let _ = dec.decode_fixed_array::<32>().ok()?;
        }
        0x01 => {
            let _ = dec.decode_compact_u32().ok()?;
        }
        0x02 => {
            let _ = dec.decode_bytes().ok()?;
        }
        0x04 => {
            let _ = dec.decode_fixed_array::<20>().ok()?;
        }
        _ => return None,
    }

    // Compact u128 value.
    let amount = u128::from(dec.decode_compact_u64().ok()?);
    Some(amount)
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Extract raw arg bytes from the first (and only) [`FieldValue::Bytes`] arg.
fn extract_raw_args(ext: &DecodedExtrinsic) -> Result<Vec<u8>> {
    match ext.args.first() {
        Some(DecodedField {
            value: FieldValue::Bytes(b),
            ..
        }) => Ok(b.clone()),
        Some(DecodedField {
            value: FieldValue::Sequence(_),
            ..
        }) => {
            // Collect all byte args.
            let mut out = Vec::new();
            for arg in &ext.args {
                if let FieldValue::Bytes(b) = &arg.value {
                    out.extend_from_slice(b);
                }
            }
            Ok(out)
        }
        None => Ok(Vec::new()),
        _ => Err(CodecError::invalid_type(
            "expected raw Bytes arg for call helper",
        )),
    }
}

/// Decode a bare call (no outer length prefix) from raw bytes.
fn decode_bare_call(bytes: &[u8]) -> Result<DecodedExtrinsic> {
    let mut dec = ScaleDecoder::new(bytes);
    let pallet_index = dec.decode_u8()?;
    let call_index = dec.decode_u8()?;
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

/// Try to extract a transfer amount from named `FieldValue::Compact` or
/// `FieldValue::U128` args.
fn extract_amount_from_named_args(ext: &DecodedExtrinsic) -> Option<u128> {
    for field in &ext.args {
        let is_amount = field
            .name
            .as_deref()
            .is_some_and(|n| matches!(n, "value" | "amount" | "keep_alive"));
        if is_amount {
            match &field.value {
                FieldValue::Compact(v) | FieldValue::U128(v) => return Some(*v),
                _ => {}
            }
        }
    }
    None
}
