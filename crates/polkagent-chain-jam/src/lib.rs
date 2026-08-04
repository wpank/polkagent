//! **EXPERIMENTAL — Maturity: Research**
//!
//! JAM testnet chain client adapter for the Polkagent platform.
//!
//! This crate provides a minimal client for querying block data from a JAM
//! testnet node. It is a leaf adapter that does **not** integrate with core
//! correctness paths (run orchestration, policy, signing, or submission).
//!
//! # Maturity
//!
//! This crate is gated behind the `jam` cargo feature in the workspace and
//! carries an **Experimental** maturity label per PRD-05 §11.5. No Polkagent
//! v1 correctness, identity, payment, or workflow claim depends on this crate.
//!
//! # Architecture
//!
//! - [`types`] — JAM-specific block and header types
//! - [`client`] — RPC client for querying a JAM testnet node
//! - [`mock`] — Deterministic mock backend for testing without a live node
//! - [`error`] — Error types for JAM client operations

#![forbid(unsafe_code)]

pub mod client;
pub mod error;
pub mod mock;
pub mod types;

pub use client::JamClient;
pub use error::JamError;
pub use mock::MockJamBackend;
pub use types::{JamBlock, JamBlockHeader, JamServiceId};

/// Maturity label for this crate.
///
/// All JAM integration surfaces must display this label to users per PRD-05
/// §11.5 principle 3.
pub const MATURITY_LABEL: &str = "Experimental";
