#![recursion_limit = "2048"]
//! SQLite storage adapter for the Polkagent platform.
//!
//! This crate provides durable, local-first storage backed by SQLite with
//! [WAL mode](https://www.sqlite.org/wal.html) for concurrent read/write access.
//!
//! # Quickstart
//!
//! ```no_run
//! use polkagent_store_sqlite::{SqlitePool, migrations};
//! use polkagent_store_sqlite::store::{SqliteRunStore, SqliteEffectStore, SqliteArtifactStore, SqliteEventStore};
//!
//! // 1. Open (or create) the database.
//! let pool = SqlitePool::open("/var/lib/polkagent/store.db").expect("open db");
//!
//! // 2. Apply any pending migrations (idempotent).
//! {
//!     let writer = pool.writer();
//!     migrations::migrate(&writer).expect("migrate");
//! }
//!
//! // 3. Construct stores.
//! let run_store      = SqliteRunStore::new(pool.clone());
//! let effect_store   = SqliteEffectStore::new(pool.clone());
//! let artifact_store = SqliteArtifactStore::new(pool.clone());
//! let event_store    = SqliteEventStore::new(pool.clone());
//! ```
//!
//! # Architecture
//!
//! ## Connection model
//!
//! [`SqlitePool`] holds one **exclusive writer** connection (behind a
//! `parking_lot::Mutex`) and can hand out independent **read-only** connections
//! on demand.  WAL mode allows readers and the single writer to proceed
//! concurrently without blocking each other.
//!
//! ## Migrations
//!
//! The [`migrations`] module provides forward-only, version-tracked schema
//! migrations.  Call [`migrations::migrate`] once at startup before any reads
//! or writes.
//!
//! ## Stores
//!
//! Four stores implement the Polkagent storage contract:
//!
//! | Store | Entities |
//! |---|---|
//! | [`store::SqliteRunStore`] | Agents, runs, turns, steps |
//! | [`store::SqliteEffectStore`] | Effect intents, attempts, outcomes |
//! | [`store::SqliteArtifactStore`] | Artifact metadata, bodies, lineage |
//! | [`store::SqliteEventStore`] | Ordered durable run events |
//!
//! # PRAGMA settings
//!
//! Every connection applies the following PRAGMAs:
//!
//! - `journal_mode = WAL` — concurrent reads/writes.
//! - `busy_timeout = 5000` — wait up to 5 s on `SQLITE_BUSY`.
//! - `synchronous = NORMAL` — durable with WAL; faster than `FULL`.
//! - `foreign_keys = ON` — FK constraint enforcement.
//! - `cache_size = -65536` — 64 MiB page cache per connection.
//! - `mmap_size = 268435456` — 256 MiB memory-mapped I/O.

pub mod artifact_store_impl;
pub mod conversation_store_impl;
pub mod error;
pub mod event_store;
// NOTE: group_store_impl and feed_store_impl are temporarily disabled.
// Their serde type graphs exceed rustc's trait-solver recursion limit
// when compiled alongside the other store modules. They will be moved
// to dedicated crates (polkagent-store-sqlite-group, -feed) to isolate
// the trait resolution. The in-memory stores in polkagent-group and
// polkagent-feed remain fully functional.
pub mod migrations;
pub mod payment_store_impl;
pub mod pool;
pub mod run_store_impl;
pub mod store;

// Re-export the most commonly used types at the crate root.
pub use error::{StoreError, StoreResult};
pub use pool::SqlitePool;
pub use store::{
    AgentRow, ArtifactRow, EffectAttemptRow, EffectIntentRow, EffectOutcomeRow, RunEventRow,
    RunRow, SqliteArtifactStore, SqliteEffectStore, SqliteEventStore, SqliteRunStore, StepRow,
    TurnRow,
};
