//! `SQLite` [`GroupStore`] implementation for the Polkagent platform.
//!
//! This crate provides a dedicated [`GroupStore`] implementation backed by
//! `SQLite`, separated from the main `polkagent-store-sqlite` crate to avoid
//! `rustc` trait-solver recursion limit issues.
//!
//! # Why a separate crate?
//!
//! The `polkagent-group` type graph — specifically [`GroupBudget`] which
//! wraps `Arc<parking_lot::Mutex<u64>>` — pushes `rustc`'s serde trait-solver
//! recursion depth when compiled alongside the other serde-heavy store
//! modules.  Isolating the [`GroupStore`] implementation here keeps each
//! compilation unit below the default limit.
//!
//! # Usage
//!
//! ```no_run
//! use polkagent_store_sqlite::SqlitePool;
//! use polkagent_store_sqlite_group::{SqliteGroupStore, migrations};
//!
//! // 1. Open (or create) the database.
//! let pool = SqlitePool::open("/var/lib/polkagent/store.db").expect("open db");
//!
//! // 2. Apply the group-store migration (idempotent).
//! {
//!     let writer = pool.writer();
//!     migrations::migrate_groups(&writer).expect("migrate");
//! }
//!
//! // 3. Wrap the pool in `SqliteGroupStore` and use it as
//! //    `Arc<dyn GroupStore>`.
//! let group_store = SqliteGroupStore::new(pool);
//! ```
//!
//! [`GroupStore`]: polkagent_group::GroupStore
//! [`GroupBudget`]: polkagent_group::types::GroupBudget

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
// Assertion-oriented unit tests unwrap controlled fixtures for precise failures.
#![cfg_attr(test, allow(clippy::expect_used, clippy::unwrap_used))]

pub mod error;
pub mod group_store_impl;
pub mod migrations;

// Re-export the newtype wrapper so callers can use it directly.
pub use group_store_impl::SqliteGroupStore;

// Re-export migration entry point for convenience.
pub use migrations::migrate_groups;
