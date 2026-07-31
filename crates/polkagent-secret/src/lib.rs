//! `polkagent-secret` — Secret management with secure storage, audit logging,
//! and zeroize-on-drop for the Polkagent platform.
//!
//! # Quick start
//!
//! ```no_run
//! use polkagent_secret::{ChainSecretStore, SecretStore, SecretId};
//!
//! # async fn example() -> polkagent_secret::Result<()> {
//! // Create a chain store (env vars -> file system).
//! let store = ChainSecretStore::new()?;
//!
//! // Look up a secret by ID.
//! let api_key = store.get(&SecretId::new("anthropic-api-key")).await?;
//! println!("key = {api_key}"); // prints "[REDACTED]"
//! # Ok(())
//! # }
//! ```
//!
//! # Architecture
//!
//! The crate provides a trait-based secret storage system with multiple
//! backends:
//!
//! | Module | Backend | Read | Write |
//! |--------|---------|------|-------|
//! | [`env`] | Environment variables | Yes | No |
//! | [`file`] | `~/.polkagent/secrets/` | Yes | Yes |
//! | [`chain`] | Composite (env -> file) | Yes | Yes |
//!
//! All access is recorded by the [`audit`] module to a JSONL log.
//!
//! # Security
//!
//! - [`SecretValue`] is zeroed from memory on drop via [`zeroize`].
//! - `Display` and `Debug` on `SecretValue` never reveal the raw value.
//! - File-backed secrets use `0600` permissions (owner-only).
//! - Audit logs never contain secret values.

// `deny` rather than `forbid` so test modules can `#[allow(unsafe_code)]`
// for `std::env::set_var` / `remove_var`, which require `unsafe` in Rust >=1.83.
#![deny(unsafe_code)]
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

pub mod audit;
pub mod chain;
pub mod env;
pub mod file;
pub mod store;
pub mod types;

// ---------------------------------------------------------------------------
// Re-exports
// ---------------------------------------------------------------------------

pub use audit::{AuditEntry, AuditOperation, SecretAuditLog};
pub use chain::ChainSecretStore;
pub use env::EnvSecretStore;
pub use file::FileSecretStore;
pub use store::{Result, SecretError, SecretStore};
pub use types::{SecretId, SecretMetadata, SecretSource, SecretValue};
