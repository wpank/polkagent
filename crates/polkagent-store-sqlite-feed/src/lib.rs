//! `SQLite` [`polkagent_feed::FeedStore`] implementation for the `Polkagent` platform.
//!
//! This crate provides a durable, SQLite-backed implementation of
//! [`polkagent_feed::FeedStore`] via the [`SqliteFeedStore`] newtype wrapper
//! around [`SqlitePool`] from `polkagent-store-sqlite`.
//!
//! It is separated from `polkagent-store-sqlite` into its own crate to keep
//! the feed-related persistence logic isolated from the core store modules.
//!
//! # Quickstart
//!
//! ```no_run
//! use polkagent_store_sqlite::SqlitePool;
//! use polkagent_store_sqlite_feed::{SqliteFeedStore, migrations};
//!
//! // 1. Open the database (typically shared with polkagent-store-sqlite).
//! let pool = SqlitePool::open("/var/lib/polkagent/store.db").expect("open db");
//!
//! // 2. Apply feed-specific migrations (idempotent).
//! {
//!     let writer = pool.writer();
//!     migrations::migrate(&writer).expect("migrate");
//! }
//!
//! // 3. Wrap the pool in SqliteFeedStore to use it as a FeedStore.
//! //    use polkagent_feed::FeedStore;
//! //    let store = SqliteFeedStore::new(pool);
//! //    let feeds = store.list_feeds().await?;
//! ```
//!
//! # Architecture
//!
//! - **[`feed_store_impl`]** -- Defines [`SqliteFeedStore`] and implements all
//!   14 [`polkagent_feed::FeedStore`] methods by wrapping synchronous `rusqlite` calls in
//!   `tokio::task::spawn_blocking`.
//! - **[`migrations`]** -- `CREATE TABLE IF NOT EXISTS` DDL for the four
//!   feed-related tables.  Safe to call even if the main store's V7 migration
//!   has already created them.
//! - **[`error`]** -- Error mapping helpers from `rusqlite`, `serde_json`,
//!   `uuid`, and timestamp parsing to [`polkagent_feed::FeedError`].

pub mod error;
pub mod feed_store_impl;
pub mod migrations;

// Re-export the wrapper and the underlying pool type for convenience.
pub use feed_store_impl::SqliteFeedStore;
pub use polkagent_store_sqlite::SqlitePool;
