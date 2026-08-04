//! `polkagent-metadata` — Metadata service for Polkadot chain metadata
//! fetching, caching, pinning, and drift detection.
//!
//! This crate provides the metadata management layer for the Polkagent
//! platform. It is a pure-Rust crate with no dependency on concrete
//! Polkadot runtimes or `subxt`.
//!
//! # Architecture
//!
//! ```text
//!  ┌─────────────────────────────────────────────┐
//!  │             MetadataService                  │
//!  │  ┌──────────┐ ┌──────────┐ ┌──────────────┐ │
//!  │  │  Cache   │ │ PinStore │ │DriftDetector │ │
//!  │  └──────────┘ └──────────┘ └──────────────┘ │
//!  └─────────────────────────────────────────────┘
//! ```
//!
//! - **Cache**: LRU in-memory cache of [`MetadataSnapshot`]s.
//! - **PinStore**: Stores hashes of known-good metadata for drift comparison.
//! - **DriftDetector**: Compares current metadata against pins.
//! - **MetadataService**: Facade composing all three.
//!
//! # Usage
//!
//! ```rust
//! use polkagent_metadata::{MetadataService, ChainId, MetadataSnapshot, MetadataVersion};
//! use polkagent_core::now;
//! use std::time::Duration;
//!
//! let svc = MetadataService::new();
//!
//! // Register a metadata snapshot fetched from the chain.
//! let snapshot = MetadataSnapshot::new(
//!     ChainId::new("polkadot"),
//!     MetadataVersion::V14,
//!     vec![0xDE, 0xAD, 0xBE, 0xEF],
//!     now(),
//!     1_000_000,
//! );
//! let drift = svc.register_snapshot(snapshot);
//! assert!(drift.is_none()); // No pins yet, so no drift.
//!
//! // Pin the current metadata as known-good.
//! svc.pin_current(&ChainId::new("polkadot"), "v1.0.0").unwrap();
//!
//! // Check if metadata is still fresh.
//! assert!(!svc.is_stale(&ChainId::new("polkadot"), Duration::from_secs(3600)));
//! ```

#![forbid(unsafe_code)]
#![warn(
    missing_docs,
    clippy::pedantic,
    clippy::unwrap_used,
    clippy::expect_used
)]
#![allow(
    clippy::module_name_repetitions,
    clippy::must_use_candidate,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc
)]

pub mod cache;
#[cfg(feature = "cached")]
pub mod cached_service;
pub mod decode;
pub mod diff;
pub mod drift;
pub mod error;
pub mod pin;
pub mod service;
pub mod types;
pub mod validation;

// ---------------------------------------------------------------------------
// Convenience re-exports
// ---------------------------------------------------------------------------

pub use cache::MetadataCache;
#[cfg(feature = "cached")]
pub use cached_service::CachedMetadataService;
pub use decode::DecodeService;
pub use diff::{diff_metadata, generate_impact_brief, is_breaking, MetadataDiff, PalletDiff};
pub use drift::DriftDetector;
pub use error::MetadataError;
pub use pin::PinStore;
pub use service::MetadataService;
pub use types::{
    CallInfo, ChainId, MetadataDrift, MetadataHash, MetadataSnapshot, MetadataVersion, PalletInfo,
    PinnedMetadata,
};
pub use validation::validate_network;
