//! `polkagent-identity` — Identity and account types for Polkadot chain
//! integration.
//!
//! This crate provides the identity primitives needed to represent and work
//! with accounts on Substrate-based chains (Polkadot, Kusama, Westend, etc.):
//!
//! - **[`AccountId32`]** — A 32-byte account identifier, displayed and
//!   serialized as hex.
//! - **[`NetworkId`]** — Identifies a network by its SS58 prefix.
//! - **[`SS58Address`]** — An SS58-encoded address string with checksum
//!   validation.
//! - **[`ChainAccount`]** — An account on a specific chain with an optional
//!   label.
//! - **[`AgentIdentity`]** — Links an agent to its on-chain accounts.
//! - **[`AgentCard`]** — Signed agent metadata for verification.
//!
//! # SS58 checksum note
//!
//! The SS58 implementation uses Blake3 for checksums rather than the canonical
//! Blake2b-512. This is a simplified implementation — production deployments
//! that need interoperability with standard Substrate tooling should use
//! Blake2b-512.
//!
//! # Module layout
//!
//! | Module | Contents |
//! |--------|----------|
//! | [`types`] | [`AccountId32`], [`NetworkId`], [`SS58Address`], [`ChainAccount`] |
//! | [`ss58`] | Low-level SS58 encoding and decoding functions |
//! | [`agent_identity`] | [`AgentIdentity`], [`AgentCard`] |
//! | [`error`] | [`IdentityError`] |

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

pub mod agent_identity;
pub mod error;
pub mod ss58;
pub mod types;

// ---------------------------------------------------------------------------
// Convenience re-exports
// ---------------------------------------------------------------------------

pub use agent_identity::{AgentCard, AgentIdentity};
pub use error::IdentityError;
pub use ss58::{decode_ss58, encode_ss58};
pub use types::{AccountId32, ChainAccount, NetworkId, SS58Address};
