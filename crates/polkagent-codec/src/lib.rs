//! `polkagent-codec` — Pure-Rust SCALE encode/decode for Polkadot extrinsics
//! and metadata types.
//!
//! This crate implements the [SCALE (Simple Concatenated Aggregate
//! Little-Endian)][scale] codec used by Substrate/Polkadot runtimes without
//! any dependency on `parity-scale-codec`, `subxt`, or any Polkadot SDK crate.
//!
//! [scale]: https://docs.substrate.io/reference/scale-codec/
//!
//! # Modules
//!
//! | Module | Purpose |
//! |--------|---------|
//! | [`error`] | [`CodecError`] enum and [`Result`] alias |
//! | [`scale`] | Core [`ScaleDecoder`] / [`ScaleEncoder`] primitives |
//! | [`metadata`] | Runtime metadata types and v14/v15 parsers |
//! | [`decode`] | Higher-level extrinsic and call decoding |
//! | [`call`]   | Helpers for well-known call types (batch, proxy, transfer) |
//!
//! # Quick start
//!
//! ```rust
//! use polkagent_codec::scale::{ScaleDecoder, ScaleEncoder};
//!
//! // Encode a compact integer.
//! let mut enc = ScaleEncoder::new();
//! enc.encode_compact_u32(16384);
//! let bytes = enc.finish();
//! assert_eq!(bytes, vec![0x02, 0x00, 0x01, 0x00]);
//!
//! // Decode it back.
//! let mut dec = ScaleDecoder::new(&bytes);
//! let v = dec.decode_compact_u32().unwrap();
//! assert_eq!(v, 16384);
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs, clippy::pedantic)]
#![allow(
    clippy::module_name_repetitions,
    clippy::must_use_candidate,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::cast_possible_truncation
)]

pub mod call;
pub mod decode;
pub mod error;
pub mod metadata;
pub mod scale;

#[cfg(test)]
mod tests;

// ---------------------------------------------------------------------------
// Top-level re-exports
// ---------------------------------------------------------------------------

pub use call::{
    decode_batch_call, decode_proxy_call, extract_transfer_amount, is_batch_call, is_proxy_call,
    is_transfer_call,
};
pub use decode::{
    decode_call_with_metadata, decode_extrinsic, DecodedExtrinsic, DecodedField, FieldValue,
};
pub use error::{CodecError, Result};
pub use metadata::{
    parse_metadata, parse_metadata_v14, parse_metadata_v15, CallMetadata, ConstantMetadata,
    EventMetadata, FieldMetadata, PalletMetadata, PrimitiveType, RuntimeMetadata, StorageMetadata,
    TypeDef, TypeRegistry, VariantDef,
};
pub use scale::{ScaleDecoder, ScaleEncoder};
