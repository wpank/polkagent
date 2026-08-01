//! Standalone migration management tool for Polkagent SQLite databases.
//!
//! This crate provides a CLI and library for running, listing, and verifying
//! database migrations independently of the main application.
//!
//! # Features
//!
//! - **Forward-only by default**: Rollback is opt-in and requires a `down.sql`.
//! - **BLAKE3 checksums**: Detect migration file tampering after initial apply.
//! - **Dry-run mode**: Preview what would change without touching the database.
//! - **Transaction per migration**: Each migration runs in its own transaction.
//! - **Lock file**: Prevents concurrent migration processes.
//!
//! # Modules
//!
//! | Module | Purpose |
//! |---|---|
//! | [`migration`] | Core data types (`Migration`, `MigrationState`) |
//! | [`runner`] | Apply pending migrations, rollback support |
//! | [`status`] | List applied/pending migrations with state |
//! | [`verify`] | Verify BLAKE3 checksums of applied migrations |
//! | [`generator`] | Generate new migration file templates |
//! | [`cli`] | CLI subcommands (clap-based) |
//! | [`error`] | Error types |
//!
//! # Example
//!
//! ```no_run
//! use polkagent_migration::migration::Migration;
//! use polkagent_migration::runner::MigrationRunner;
//! use rusqlite::Connection;
//!
//! let conn = Connection::open("my.db").expect("open db");
//! let runner = MigrationRunner::new(None);
//!
//! let migrations = vec![
//!     Migration::new(1, "create users", "CREATE TABLE users (id INTEGER PRIMARY KEY);"),
//!     Migration::new(2, "add email", "ALTER TABLE users ADD COLUMN email TEXT;"),
//! ];
//!
//! let results = runner.apply_pending(&conn, &migrations, false).expect("migrate");
//! for r in &results {
//!     println!("Applied v{}: {}", r.version, r.name);
//! }
//! ```

pub mod cli;
pub mod error;
pub mod generator;
pub mod migration;
pub mod runner;
pub mod status;
pub mod verify;

// Re-export key types at crate root for convenience.
pub use error::{MigrationError, MigrationResult};
pub use migration::{Migration, MigrationState};
pub use runner::MigrationRunner;
