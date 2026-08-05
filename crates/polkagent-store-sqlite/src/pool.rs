//! `SQLite` connection pool with WAL mode.
//!
//! Polkagent uses a single-writer / multiple-reader model that maps cleanly
//! to `SQLite`'s WAL mode:
//!
//! - **One writer** — guarded by a `parking_lot::Mutex`.  Callers call
//!   [`SqlitePool::writer`] to obtain an exclusive-writer guard.  Only one
//!   writer may hold the guard at a time; others wait.
//! - **Multiple readers** — each call to [`SqlitePool::reader`] opens a fresh
//!   read-only connection (in WAL mode readers never block writers and vice
//!   versa).
//!
//! # PRAGMA settings applied to every connection
//!
//! | PRAGMA | Value | Reason |
//! |---|---|---|
//! | `journal_mode` | `WAL` | Concurrent read/write; readers don't block writers |
//! | `busy_timeout` | `5000` ms | Wait up to 5 s before returning `SQLITE_BUSY` |
//! | `synchronous` | `NORMAL` | Durable on power-loss with WAL; faster than `FULL` |
//! | `foreign_keys` | `ON` | Enforce FK constraints |
//! | `cache_size` | `-65536` | 64 MB page cache per connection |
//! | `mmap_size` | `268435456` | 256 MB memory-mapped I/O |

use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::{Mutex, MutexGuard};
use rusqlite::{Connection, OpenFlags};
use tracing::debug;

use crate::error::{StoreError, StoreResult};

/// The shared connection pool.
///
/// Clone is cheap (`Arc` under the hood).
#[derive(Clone)]
pub struct SqlitePool {
    inner: Arc<PoolInner>,
}

struct PoolInner {
    db_path: PathBuf,
    writer: Mutex<Connection>,
}

impl std::fmt::Debug for SqlitePool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqlitePool")
            .field("db_path", &self.inner.db_path)
            .finish_non_exhaustive()
    }
}

impl SqlitePool {
    /// Open (or create) the database at `path` and configure WAL mode.
    ///
    /// This must be called once at startup.  All subsequent connections
    /// (readers) are opened against the same file with the same PRAGMAs.
    pub fn open(path: impl AsRef<Path>) -> StoreResult<Self> {
        let path = path.as_ref().to_path_buf();
        debug!(db_path = %path.display(), "opening sqlite pool");

        let writer_conn = open_connection(&path, false)?;
        // WAL must be activated on the first connection; readers inherit it.
        writer_conn.execute_batch("PRAGMA journal_mode = WAL;")?;

        Ok(Self {
            inner: Arc::new(PoolInner {
                db_path: path,
                writer: Mutex::new(writer_conn),
            }),
        })
    }

    /// Open an in-memory database (useful for tests).
    pub fn open_in_memory() -> StoreResult<Self> {
        let conn = Connection::open_in_memory()?;
        apply_pragmas(&conn)?;
        // WAL mode is not supported for in-memory databases; leave it as the
        // default (DELETE journal).  Tests that verify WAL mode should use a
        // file-based path.
        Ok(Self {
            inner: Arc::new(PoolInner {
                db_path: PathBuf::from(":memory:"),
                writer: Mutex::new(conn),
            }),
        })
    }

    /// Acquire the exclusive writer connection.
    ///
    /// Only one caller holds this guard at a time; all others block.
    /// The guard is released when it is dropped.
    pub fn writer(&self) -> MutexGuard<'_, Connection> {
        self.inner.writer.lock()
    }

    /// Open a fresh read-only connection to the database.
    ///
    /// In WAL mode, read connections do not block the writer and the writer
    /// does not block readers.
    pub fn reader(&self) -> StoreResult<Connection> {
        if self.inner.db_path == Path::new(":memory:") {
            // For in-memory databases, return a second connection to the same
            // writer connection isn't possible; return an error or fall back.
            // In practice tests that need concurrent readers should use a file.
            Err(StoreError::Pool(
                "cannot open a separate reader for an in-memory database".into(),
            ))
        } else {
            open_connection(&self.inner.db_path, true)
        }
    }

    /// Return the filesystem path of the database.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.inner.db_path
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn open_connection(path: &Path, read_only: bool) -> StoreResult<Connection> {
    let flags = if read_only {
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX
    } else {
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
    };

    let conn = Connection::open_with_flags(path, flags)?;
    apply_pragmas(&conn)?;
    Ok(conn)
}

fn apply_pragmas(conn: &Connection) -> StoreResult<()> {
    conn.execute_batch(
        "
        PRAGMA busy_timeout    = 5000;
        PRAGMA synchronous     = NORMAL;
        PRAGMA foreign_keys    = ON;
        PRAGMA cache_size      = -65536;
        PRAGMA mmap_size       = 268435456;
        ",
    )?;
    Ok(())
}
