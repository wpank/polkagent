//! `SQLite`-backed implementation of [`MemoryStore`].
//!
//! Uses a single writer connection protected by a `parking_lot::Mutex` and
//! WAL mode for concurrent read access. The schema includes an FTS5 virtual
//! table for full-text search on memory content.

use std::fmt::Write as _;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use rusqlite::{params, Connection, OptionalExtension};
use tracing::debug;

use polkagent_core::ids::AgentId;

use crate::classification::Classification;
use crate::error::{MemoryError, MemoryResult};
use crate::store::MemoryStore;
use crate::types::{
    Episode, EpisodeId, MemoryEntry, MemoryId, MemoryProvenance, MemoryQuery, MemoryType,
};

// ---------------------------------------------------------------------------
// Schema
// ---------------------------------------------------------------------------

const SCHEMA_SQL: &str = "
CREATE TABLE IF NOT EXISTS memories (
    id              TEXT PRIMARY KEY,
    agent_id        TEXT NOT NULL,
    episode_id      TEXT,
    memory_type     TEXT NOT NULL,
    content         TEXT NOT NULL,
    embedding       BLOB,
    metadata        TEXT NOT NULL DEFAULT '{}',
    provenance      TEXT,
    created_at      TEXT NOT NULL,
    accessed_at     TEXT NOT NULL,
    access_count    INTEGER NOT NULL DEFAULT 0,
    relevance_score REAL NOT NULL DEFAULT 1.0,
    confidence      REAL NOT NULL DEFAULT 1.0,
    classification  TEXT NOT NULL DEFAULT 'internal'
);

CREATE INDEX IF NOT EXISTS idx_memories_agent_id ON memories(agent_id);
CREATE INDEX IF NOT EXISTS idx_memories_episode_id ON memories(episode_id);
CREATE INDEX IF NOT EXISTS idx_memories_memory_type ON memories(memory_type);
CREATE INDEX IF NOT EXISTS idx_memories_relevance ON memories(relevance_score DESC);
CREATE INDEX IF NOT EXISTS idx_memories_created_at ON memories(created_at);

CREATE TABLE IF NOT EXISTS episodes (
    id          TEXT PRIMARY KEY,
    agent_id    TEXT NOT NULL,
    title       TEXT NOT NULL,
    summary     TEXT,
    started_at  TEXT NOT NULL,
    ended_at    TEXT,
    turn_count  INTEGER NOT NULL DEFAULT 0,
    metadata    TEXT NOT NULL DEFAULT '{}'
);

CREATE INDEX IF NOT EXISTS idx_episodes_agent_id ON episodes(agent_id);

CREATE TABLE IF NOT EXISTS memory_provenance (
    memory_id           TEXT PRIMARY KEY REFERENCES memories(id) ON DELETE CASCADE,
    source_run_id       TEXT,
    source_turn         INTEGER,
    extraction_method   TEXT NOT NULL,
    confidence          REAL NOT NULL DEFAULT 1.0,
    verified            INTEGER NOT NULL DEFAULT 0,
    source_artifact_id  TEXT,
    source_agent_id     TEXT,
    ingested_at         TEXT
);
";

/// SQL to create the FTS5 virtual table for full-text search on memory content.
const FTS5_SQL: &str = "
CREATE VIRTUAL TABLE IF NOT EXISTS memories_fts USING fts5(
    content,
    content='memories',
    content_rowid='rowid'
);

-- Triggers to keep the FTS index in sync with the memories table.
CREATE TRIGGER IF NOT EXISTS memories_ai AFTER INSERT ON memories BEGIN
    INSERT INTO memories_fts(rowid, content) VALUES (new.rowid, new.content);
END;

CREATE TRIGGER IF NOT EXISTS memories_ad AFTER DELETE ON memories BEGIN
    INSERT INTO memories_fts(memories_fts, rowid, content)
        VALUES('delete', old.rowid, old.content);
END;

CREATE TRIGGER IF NOT EXISTS memories_au AFTER UPDATE ON memories BEGIN
    INSERT INTO memories_fts(memories_fts, rowid, content)
        VALUES('delete', old.rowid, old.content);
    INSERT INTO memories_fts(rowid, content) VALUES (new.rowid, new.content);
END;
";

// ---------------------------------------------------------------------------
// SqliteMemoryStore
// ---------------------------------------------------------------------------

/// `SQLite`-backed memory store.
///
/// Thread-safe via a `parking_lot::Mutex` around the writer connection.
/// Clone is cheap (`Arc` under the hood).
#[derive(Clone)]
pub struct SqliteMemoryStore {
    inner: Arc<StoreInner>,
}

struct StoreInner {
    conn: Mutex<Connection>,
    fts_available: bool,
}

impl std::fmt::Debug for SqliteMemoryStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteMemoryStore")
            .field("fts_available", &self.inner.fts_available)
            .finish_non_exhaustive()
    }
}

impl SqliteMemoryStore {
    /// Open (or create) a file-backed memory store at the given path.
    pub fn open(path: impl AsRef<Path>) -> MemoryResult<Self> {
        let path = path.as_ref();
        debug!(db_path = %path.display(), "opening memory store");

        let conn = Connection::open(path)?;
        Self::init(conn)
    }

    /// Create an in-memory memory store (useful for tests).
    pub fn open_in_memory() -> MemoryResult<Self> {
        let conn = Connection::open_in_memory()?;
        Self::init(conn)
    }

    fn init(conn: Connection) -> MemoryResult<Self> {
        // Apply pragmas.
        conn.execute_batch(
            "
            PRAGMA journal_mode = WAL;
            PRAGMA busy_timeout = 5000;
            PRAGMA synchronous  = NORMAL;
            PRAGMA foreign_keys = ON;
            ",
        )?;

        // Apply core schema.
        conn.execute_batch(SCHEMA_SQL)?;

        // Apply incremental migrations for databases created before the
        // current schema version. Each migration is idempotent: it checks
        // whether the change is already present before applying it.
        Self::migrate(&conn)?;

        // Attempt to create FTS5 virtual table. Not all SQLite builds include
        // FTS5, so we gracefully degrade to LIKE-based search if it fails.
        let fts_available = conn.execute_batch(FTS5_SQL).is_ok();
        if !fts_available {
            debug!("FTS5 not available; falling back to LIKE search");
        }

        Ok(Self {
            inner: Arc::new(StoreInner {
                conn: Mutex::new(conn),
                fts_available,
            }),
        })
    }

    /// Apply incremental schema migrations.
    ///
    /// Each migration is guarded so it is safe to run against both fresh and
    /// existing databases. New migrations must be appended in order.
    fn migrate(conn: &Connection) -> MemoryResult<()> {
        // --- Migration 1 (P4-8): provenance columns & index ---
        //
        // The initial schema for `memory_provenance` had only:
        //   memory_id, source_run_id, source_turn, extraction_method,
        //   confidence, verified
        //
        // P4-8 added three new columns and an index on one of them.
        // Databases created before P4-8 need the columns added first;
        // otherwise the index creation fails with "no such column".
        //
        // SQLite does not support `ADD COLUMN IF NOT EXISTS`, so we inspect
        // `PRAGMA table_info` and only issue the ALTER TABLE when necessary.
        let existing_columns: Vec<String> = {
            let mut stmt = conn.prepare("PRAGMA table_info(memory_provenance)")?;
            let names = stmt
                .query_map([], |row| row.get::<_, String>(1))?
                .collect::<Result<Vec<_>, _>>()?;
            names
        };

        if !existing_columns.iter().any(|c| c == "source_artifact_id") {
            conn.execute_batch(
                "ALTER TABLE memory_provenance ADD COLUMN source_artifact_id TEXT;",
            )?;
        }
        if !existing_columns.iter().any(|c| c == "source_agent_id") {
            conn.execute_batch("ALTER TABLE memory_provenance ADD COLUMN source_agent_id TEXT;")?;
        }
        if !existing_columns.iter().any(|c| c == "ingested_at") {
            conn.execute_batch("ALTER TABLE memory_provenance ADD COLUMN ingested_at TEXT;")?;
        }

        // Create the index now that the column is guaranteed to exist.
        conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_provenance_artifact \
             ON memory_provenance(source_artifact_id);",
        )?;

        Ok(())
    }

    /// Whether FTS5 full-text search is available.
    #[must_use]
    pub fn fts_available(&self) -> bool {
        self.inner.fts_available
    }
}

// ---------------------------------------------------------------------------
// MemoryStore implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl MemoryStore for SqliteMemoryStore {
    async fn store_memory(&self, entry: &MemoryEntry) -> MemoryResult<MemoryId> {
        let conn = self.inner.conn.lock();

        let embedding_bytes: Option<Vec<u8>> = entry
            .embedding
            .as_ref()
            .map(|v| v.iter().flat_map(|f| f.to_le_bytes()).collect());

        let provenance_json: Option<String> = entry
            .provenance
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;

        conn.execute(
            "INSERT INTO memories (id, agent_id, episode_id, memory_type, content, embedding,
                                   metadata, provenance, created_at, accessed_at, access_count,
                                   relevance_score, confidence, classification)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                entry.id.to_string(),
                entry.agent_id.to_string(),
                entry.episode_id.map(|e| e.to_string()),
                entry.memory_type.to_string(),
                entry.content,
                embedding_bytes,
                entry.metadata.to_string(),
                provenance_json,
                entry.created_at.to_rfc3339(),
                entry.accessed_at.to_rfc3339(),
                entry.access_count,
                entry.relevance_score,
                entry.confidence,
                entry.classification.to_string(),
            ],
        )?;

        // Store provenance separately for queryability.
        if let Some(ref prov) = entry.provenance {
            conn.execute(
                "INSERT OR REPLACE INTO memory_provenance
                     (memory_id, source_run_id, source_turn, extraction_method, confidence, verified,
                      source_artifact_id, source_agent_id, ingested_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    entry.id.to_string(),
                    prov.source_run_id,
                    prov.source_turn,
                    prov.extraction_method,
                    prov.confidence,
                    i32::from(prov.verified),
                    prov.source_artifact_id,
                    prov.source_agent_id,
                    prov.ingested_at.map(|t| t.to_rfc3339()),
                ],
            )?;
        }

        Ok(entry.id)
    }

    async fn get_memory(&self, id: MemoryId) -> MemoryResult<MemoryEntry> {
        let conn = self.inner.conn.lock();

        // Increment access count and update accessed_at.
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE memories SET access_count = access_count + 1, accessed_at = ?1 WHERE id = ?2",
            params![now, id.to_string()],
        )?;

        conn.query_row(
            "SELECT id, agent_id, episode_id, memory_type, content, embedding,
                    metadata, provenance, created_at, accessed_at, access_count,
                    relevance_score, confidence, classification
             FROM memories WHERE id = ?1",
            params![id.to_string()],
            row_to_memory,
        )
        .optional()?
        .ok_or_else(|| MemoryError::NotFound(format!("memory {id}")))?
    }

    async fn search(&self, query: &MemoryQuery) -> MemoryResult<Vec<MemoryEntry>> {
        let conn = self.inner.conn.lock();

        if self.inner.fts_available && !query.query_text.is_empty() {
            search_fts(&conn, query, None)
        } else {
            search_like(&conn, query, None)
        }
    }

    async fn search_with_classification(
        &self,
        query: &MemoryQuery,
        max_classification: Classification,
    ) -> MemoryResult<Vec<MemoryEntry>> {
        let conn = self.inner.conn.lock();

        if self.inner.fts_available && !query.query_text.is_empty() {
            search_fts(&conn, query, Some(max_classification))
        } else {
            search_like(&conn, query, Some(max_classification))
        }
    }

    async fn update_relevance(&self, id: MemoryId, score: f64) -> MemoryResult<()> {
        let conn = self.inner.conn.lock();
        let rows = conn.execute(
            "UPDATE memories SET relevance_score = ?1 WHERE id = ?2",
            params![score, id.to_string()],
        )?;
        if rows == 0 {
            return Err(MemoryError::NotFound(format!("memory {id}")));
        }
        Ok(())
    }

    async fn delete_memory(&self, id: MemoryId) -> MemoryResult<()> {
        let conn = self.inner.conn.lock();
        let rows = conn.execute(
            "DELETE FROM memories WHERE id = ?1",
            params![id.to_string()],
        )?;
        if rows == 0 {
            return Err(MemoryError::NotFound(format!("memory {id}")));
        }
        Ok(())
    }

    async fn forget(&self, artifact_id: &str) -> MemoryResult<usize> {
        let conn = self.inner.conn.lock();
        // Use a SAVEPOINT so both FTS5 and primary table are updated atomically.
        conn.execute_batch("SAVEPOINT forget_artifact")?;

        let result = (|| -> MemoryResult<usize> {
            let rows = conn.execute(
                "DELETE FROM memories WHERE id IN (
                     SELECT memory_id FROM memory_provenance
                     WHERE source_artifact_id = ?1
                 )",
                params![artifact_id],
            )?;
            Ok(rows)
        })();

        match result {
            Ok(count) => {
                conn.execute_batch("RELEASE forget_artifact")?;
                Ok(count)
            }
            Err(e) => {
                let _ = conn.execute_batch("ROLLBACK TO forget_artifact");
                Err(e)
            }
        }
    }

    #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
    async fn count_entries(&self, agent_id: &AgentId) -> MemoryResult<usize> {
        let conn = self.inner.conn.lock();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM memories WHERE agent_id = ?1",
            params![agent_id.to_string()],
            |row| row.get(0),
        )?;
        Ok(count.max(0) as usize)
    }

    #[allow(clippy::cast_possible_wrap)]
    async fn list_entries(
        &self,
        agent_id: &AgentId,
        limit: usize,
        offset: usize,
    ) -> MemoryResult<Vec<MemoryEntry>> {
        let conn = self.inner.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, agent_id, episode_id, memory_type, content, embedding,
                    metadata, provenance, created_at, accessed_at, access_count,
                    relevance_score, confidence, classification
             FROM memories
             WHERE agent_id = ?1
             ORDER BY created_at ASC
             LIMIT ?2 OFFSET ?3",
        )?;

        let rows = stmt
            .query_map(
                params![agent_id.to_string(), limit as i64, offset as i64],
                row_to_memory,
            )?
            .collect::<Result<Vec<_>, _>>()?;

        let mut entries = Vec::new();
        for r in rows {
            entries.push(r?);
        }
        Ok(entries)
    }

    async fn delete_by_age(
        &self,
        agent_id: &AgentId,
        max_age: chrono::Duration,
    ) -> MemoryResult<usize> {
        let cutoff = (Utc::now() - max_age).to_rfc3339();
        let conn = self.inner.conn.lock();
        let rows = conn.execute(
            "DELETE FROM memories WHERE agent_id = ?1 AND created_at < ?2",
            params![agent_id.to_string(), cutoff],
        )?;
        Ok(rows)
    }

    async fn create_episode(&self, episode: &Episode) -> MemoryResult<EpisodeId> {
        let conn = self.inner.conn.lock();
        conn.execute(
            "INSERT INTO episodes (id, agent_id, title, summary, started_at, ended_at,
                                   turn_count, metadata)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                episode.id.to_string(),
                episode.agent_id.to_string(),
                episode.title,
                episode.summary,
                episode.started_at.to_rfc3339(),
                episode.ended_at.map(|t| t.to_rfc3339()),
                episode.turn_count,
                episode.metadata.to_string(),
            ],
        )?;
        Ok(episode.id)
    }

    async fn get_episode(&self, id: EpisodeId) -> MemoryResult<Episode> {
        let conn = self.inner.conn.lock();
        conn.query_row(
            "SELECT id, agent_id, title, summary, started_at, ended_at, turn_count, metadata
             FROM episodes WHERE id = ?1",
            params![id.to_string()],
            row_to_episode,
        )
        .optional()?
        .ok_or_else(|| MemoryError::NotFound(format!("episode {id}")))?
    }

    async fn end_episode(&self, id: EpisodeId, summary: &str) -> MemoryResult<()> {
        let conn = self.inner.conn.lock();
        let now = Utc::now().to_rfc3339();
        let rows = conn.execute(
            "UPDATE episodes SET summary = ?1, ended_at = ?2 WHERE id = ?3",
            params![summary, now, id.to_string()],
        )?;
        if rows == 0 {
            return Err(MemoryError::NotFound(format!("episode {id}")));
        }
        Ok(())
    }

    #[allow(clippy::cast_possible_wrap)]
    async fn list_episodes(&self, agent_id: AgentId, limit: usize) -> MemoryResult<Vec<Episode>> {
        let conn = self.inner.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, agent_id, title, summary, started_at, ended_at, turn_count, metadata
             FROM episodes
             WHERE agent_id = ?1
             ORDER BY started_at DESC
             LIMIT ?2",
        )?;

        let rows = stmt
            .query_map(params![agent_id.to_string(), limit as i64], row_to_episode)?
            .collect::<Result<Vec<_>, _>>()?;

        let mut episodes = Vec::new();
        for r in rows {
            episodes.push(r?);
        }
        Ok(episodes)
    }
}

// ---------------------------------------------------------------------------
// Row mappers
// ---------------------------------------------------------------------------

fn row_to_memory(row: &rusqlite::Row<'_>) -> rusqlite::Result<MemoryResult<MemoryEntry>> {
    let id_str: String = row.get(0)?;
    let agent_str: String = row.get(1)?;
    let episode_str: Option<String> = row.get(2)?;
    let type_str: String = row.get(3)?;
    let content: String = row.get(4)?;
    let embedding_bytes: Option<Vec<u8>> = row.get(5)?;
    let metadata_str: String = row.get(6)?;
    let provenance_str: Option<String> = row.get(7)?;
    let created_str: String = row.get(8)?;
    let accessed_str: String = row.get(9)?;
    let access_count: u64 = row.get(10)?;
    let relevance_score: f64 = row.get(11)?;
    let confidence: f64 = row.get(12)?;
    let classification_str: String = row.get(13)?;

    Ok((|| -> MemoryResult<MemoryEntry> {
        let id = id_str.parse::<MemoryId>()?;
        let agent_id: AgentId = agent_str.parse().map_err(MemoryError::InvalidId)?;
        let episode_id = episode_str.map(|s| s.parse::<EpisodeId>()).transpose()?;
        let memory_type: MemoryType = type_str.parse().map_err(MemoryError::InvalidOperation)?;

        let embedding = embedding_bytes.map(|bytes| {
            bytes
                .chunks_exact(4)
                .map(|chunk| {
                    let arr: [u8; 4] = chunk.try_into().unwrap_or_default();
                    f32::from_le_bytes(arr)
                })
                .collect()
        });

        let metadata: serde_json::Value = serde_json::from_str(&metadata_str)?;
        let provenance: Option<MemoryProvenance> = provenance_str
            .map(|s| serde_json::from_str(&s))
            .transpose()?;
        let created_at = parse_timestamp(&created_str)?;
        let accessed_at = parse_timestamp(&accessed_str)?;
        let classification: Classification = classification_str
            .parse()
            .map_err(MemoryError::InvalidOperation)?;

        Ok(MemoryEntry {
            id,
            agent_id,
            episode_id,
            memory_type,
            content,
            embedding,
            metadata,
            provenance,
            created_at,
            accessed_at,
            access_count,
            relevance_score,
            confidence,
            classification,
        })
    })())
}

fn row_to_episode(row: &rusqlite::Row<'_>) -> rusqlite::Result<MemoryResult<Episode>> {
    let id_str: String = row.get(0)?;
    let agent_str: String = row.get(1)?;
    let title: String = row.get(2)?;
    let summary: Option<String> = row.get(3)?;
    let started_str: String = row.get(4)?;
    let ended_str: Option<String> = row.get(5)?;
    let turn_count: u32 = row.get(6)?;
    let metadata_str: String = row.get(7)?;

    Ok((|| -> MemoryResult<Episode> {
        let id = id_str.parse::<EpisodeId>()?;
        let agent_id: AgentId = agent_str.parse().map_err(MemoryError::InvalidId)?;
        let started_at = parse_timestamp(&started_str)?;
        let ended_at = ended_str.map(|s| parse_timestamp(&s)).transpose()?;
        let metadata: serde_json::Value = serde_json::from_str(&metadata_str)?;

        Ok(Episode {
            id,
            agent_id,
            title,
            summary,
            started_at,
            ended_at,
            turn_count,
            metadata,
        })
    })())
}

fn parse_timestamp(s: &str) -> MemoryResult<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|_| MemoryError::InvalidTimestamp(s.to_string()))
}

// ---------------------------------------------------------------------------
// Classification helper
// ---------------------------------------------------------------------------

/// Returns the string labels for all Classification variants that are
/// numerically `<=` `max`. Used to build SQL `IN (...)` clauses.
fn classifications_up_to(max: Classification) -> Vec<&'static str> {
    const ALL: &[(Classification, &str)] = &[
        (Classification::Public, "public"),
        (Classification::Internal, "internal"),
        (Classification::Confidential, "confidential"),
        (Classification::Restricted, "restricted"),
    ];
    ALL.iter()
        .filter(|(c, _)| *c <= max)
        .map(|(_, s)| *s)
        .collect()
}

// ---------------------------------------------------------------------------
// Search helpers
// ---------------------------------------------------------------------------

#[allow(clippy::cast_possible_wrap)]
fn search_fts(
    conn: &Connection,
    query: &MemoryQuery,
    max_classification: Option<Classification>,
) -> MemoryResult<Vec<MemoryEntry>> {
    // Build the FTS5 query. We join through the FTS table to get ranked results.
    let mut sql = String::from(
        "SELECT m.id, m.agent_id, m.episode_id, m.memory_type, m.content, m.embedding,
                m.metadata, m.provenance, m.created_at, m.accessed_at, m.access_count,
                m.relevance_score, m.confidence, m.classification
         FROM memories m
         JOIN memories_fts fts ON m.rowid = fts.rowid
         WHERE fts.memories_fts MATCH ?1",
    );
    let mut param_idx = 2u32;

    if query.agent_id.is_some() {
        let _ = write!(sql, " AND m.agent_id = ?{param_idx}");
        param_idx += 1;
    }

    // Build dynamic WHERE clauses and collect params as strings.
    let mut extra_params: Vec<String> = Vec::new();

    if let Some(ref types) = query.memory_types {
        if !types.is_empty() {
            let placeholders: Vec<String> = types
                .iter()
                .map(|t| {
                    extra_params.push(t.to_string());
                    let p = format!("?{param_idx}");
                    param_idx += 1;
                    p
                })
                .collect();
            let _ = write!(sql, " AND m.memory_type IN ({})", placeholders.join(", "));
        }
    }

    if let Some(min_rel) = query.min_relevance {
        extra_params.push(min_rel.to_string());
        let _ = write!(sql, " AND m.relevance_score >= ?{param_idx}");
        param_idx += 1;
    }

    if let Some(ref since) = query.since {
        extra_params.push(since.to_rfc3339());
        let _ = write!(sql, " AND m.created_at >= ?{param_idx}");
        param_idx += 1;
    }

    if let Some(ref ep_id) = query.episode_id {
        extra_params.push(ep_id.to_string());
        let _ = write!(sql, " AND m.episode_id = ?{param_idx}");
        param_idx += 1;
    }

    if let Some(max_cls) = max_classification {
        let allowed = classifications_up_to(max_cls);
        if !allowed.is_empty() {
            let placeholders: Vec<String> = allowed
                .iter()
                .map(|c| {
                    extra_params.push((*c).to_string());
                    let p = format!("?{param_idx}");
                    param_idx += 1;
                    p
                })
                .collect();
            let _ = write!(
                sql,
                " AND m.classification IN ({})",
                placeholders.join(", ")
            );
        }
    }

    sql.push_str(" ORDER BY m.relevance_score DESC, rank LIMIT ?100");

    // Build a dynamic rusqlite params vector.
    // Sanitize the FTS5 query to prevent metacharacter injection.  FTS5 supports
    // operators like `"`, `*`, `NEAR(…)` and column filters (`content:`) that could
    // be used to probe memory contents beyond the caller's intended search scope.
    // Each whitespace-separated word is individually quoted to neutralise operators
    // while still allowing matches when the words appear in different positions.
    let sanitized_query = query
        .query_text
        .split_whitespace()
        .map(|w| format!("\"{}\"", w.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" ");
    let mut values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    values.push(Box::new(sanitized_query));
    if let Some(ref agent_id) = query.agent_id {
        values.push(Box::new(agent_id.to_string()));
    }
    for p in &extra_params {
        values.push(Box::new(p.clone()));
    }
    values.push(Box::new(query.limit as i64));

    // Fix the LIMIT placeholder -- we used ?100 as a marker, replace with actual index.
    let limit_idx = values.len();
    sql = sql.replace("?100", &format!("?{limit_idx}"));

    let params_refs: Vec<&dyn rusqlite::types::ToSql> =
        values.iter().map(std::convert::AsRef::as_ref).collect();

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(params_refs.as_slice(), row_to_memory)?
        .collect::<Result<Vec<_>, _>>()?;

    let mut results = Vec::new();
    for r in rows {
        results.push(r?);
    }
    Ok(results)
}

#[allow(clippy::cast_possible_wrap)]
fn search_like(
    conn: &Connection,
    query: &MemoryQuery,
    max_classification: Option<Classification>,
) -> MemoryResult<Vec<MemoryEntry>> {
    let mut sql = String::from(
        "SELECT id, agent_id, episode_id, memory_type, content, embedding,
                metadata, provenance, created_at, accessed_at, access_count,
                relevance_score, confidence, classification
         FROM memories
         WHERE 1=1",
    );
    let mut param_idx = 1u32;
    let mut extra_params: Vec<String> = Vec::new();

    if query.agent_id.is_some() {
        let _ = write!(sql, " AND agent_id = ?{param_idx}");
        param_idx += 1;
    }

    if !query.query_text.is_empty() {
        extra_params.push(format!("%{}%", query.query_text));
        let _ = write!(sql, " AND content LIKE ?{param_idx}");
        param_idx += 1;
    }

    if let Some(ref types) = query.memory_types {
        if !types.is_empty() {
            let placeholders: Vec<String> = types
                .iter()
                .map(|t| {
                    extra_params.push(t.to_string());
                    let p = format!("?{param_idx}");
                    param_idx += 1;
                    p
                })
                .collect();
            let _ = write!(sql, " AND memory_type IN ({})", placeholders.join(", "));
        }
    }

    if let Some(min_rel) = query.min_relevance {
        extra_params.push(min_rel.to_string());
        let _ = write!(sql, " AND relevance_score >= ?{param_idx}");
        param_idx += 1;
    }

    if let Some(ref since) = query.since {
        extra_params.push(since.to_rfc3339());
        let _ = write!(sql, " AND created_at >= ?{param_idx}");
        param_idx += 1;
    }

    if let Some(ref ep_id) = query.episode_id {
        extra_params.push(ep_id.to_string());
        let _ = write!(sql, " AND episode_id = ?{param_idx}");
        param_idx += 1;
    }

    if let Some(max_cls) = max_classification {
        let allowed = classifications_up_to(max_cls);
        if !allowed.is_empty() {
            let placeholders: Vec<String> = allowed
                .iter()
                .map(|c| {
                    extra_params.push((*c).to_string());
                    let p = format!("?{param_idx}");
                    param_idx += 1;
                    p
                })
                .collect();
            let _ = write!(sql, " AND classification IN ({})", placeholders.join(", "));
        }
    }

    sql.push_str(" ORDER BY relevance_score DESC LIMIT ?100");

    let mut values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    if let Some(ref agent_id) = query.agent_id {
        values.push(Box::new(agent_id.to_string()));
    }
    for p in &extra_params {
        values.push(Box::new(p.clone()));
    }
    values.push(Box::new(query.limit as i64));

    let limit_idx = values.len();
    sql = sql.replace("?100", &format!("?{limit_idx}"));

    let params_refs: Vec<&dyn rusqlite::types::ToSql> =
        values.iter().map(std::convert::AsRef::as_ref).collect();

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(params_refs.as_slice(), row_to_memory)?
        .collect::<Result<Vec<_>, _>>()?;

    let mut results = Vec::new();
    for r in rows {
        results.push(r?);
    }
    Ok(results)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::MemoryProvenance;

    fn make_agent_id() -> AgentId {
        AgentId::new()
    }

    fn make_entry(agent_id: AgentId, content: &str, mt: MemoryType) -> MemoryEntry {
        let now = Utc::now();
        MemoryEntry {
            id: MemoryId::new(),
            agent_id,
            episode_id: None,
            memory_type: mt,
            content: content.to_string(),
            embedding: None,
            metadata: serde_json::json!({}),
            provenance: None,
            created_at: now,
            accessed_at: now,
            access_count: 0,
            relevance_score: 1.0,
            confidence: 1.0,
            classification: Classification::default(),
        }
    }

    #[tokio::test]
    async fn store_and_get_memory() {
        let store = SqliteMemoryStore::open_in_memory().unwrap();
        let agent = make_agent_id();
        let entry = make_entry(agent, "The sky is blue", MemoryType::Semantic);
        let id = entry.id;

        store.store_memory(&entry).await.unwrap();
        let retrieved = store.get_memory(id).await.unwrap();

        assert_eq!(retrieved.id, id);
        assert_eq!(retrieved.content, "The sky is blue");
        assert_eq!(retrieved.memory_type, MemoryType::Semantic);
        assert_eq!(retrieved.access_count, 1); // incremented on get
    }

    #[tokio::test]
    async fn get_nonexistent_memory_returns_not_found() {
        let store = SqliteMemoryStore::open_in_memory().unwrap();
        let result = store.get_memory(MemoryId::new()).await;
        assert!(matches!(result, Err(MemoryError::NotFound(_))));
    }

    #[tokio::test]
    async fn delete_memory_works() {
        let store = SqliteMemoryStore::open_in_memory().unwrap();
        let agent = make_agent_id();
        let entry = make_entry(agent, "to be deleted", MemoryType::Episodic);
        let id = entry.id;

        store.store_memory(&entry).await.unwrap();
        store.delete_memory(id).await.unwrap();

        let result = store.get_memory(id).await;
        assert!(matches!(result, Err(MemoryError::NotFound(_))));
    }

    #[tokio::test]
    async fn delete_nonexistent_memory_returns_not_found() {
        let store = SqliteMemoryStore::open_in_memory().unwrap();
        let result = store.delete_memory(MemoryId::new()).await;
        assert!(matches!(result, Err(MemoryError::NotFound(_))));
    }

    #[tokio::test]
    async fn update_relevance_score() {
        let store = SqliteMemoryStore::open_in_memory().unwrap();
        let agent = make_agent_id();
        let entry = make_entry(agent, "fact", MemoryType::Semantic);
        let id = entry.id;

        store.store_memory(&entry).await.unwrap();
        store.update_relevance(id, 0.5).await.unwrap();

        let retrieved = store.get_memory(id).await.unwrap();
        assert!((retrieved.relevance_score - 0.5).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn search_with_like_fallback() {
        let store = SqliteMemoryStore::open_in_memory().unwrap();
        let agent = make_agent_id();

        let e1 = make_entry(
            agent,
            "Rust is a systems programming language",
            MemoryType::Semantic,
        );
        let e2 = make_entry(agent, "Python is great for scripting", MemoryType::Semantic);
        let e3 = make_entry(
            agent,
            "Rust has zero-cost abstractions",
            MemoryType::Procedural,
        );

        store.store_memory(&e1).await.unwrap();
        store.store_memory(&e2).await.unwrap();
        store.store_memory(&e3).await.unwrap();

        let query = MemoryQuery {
            agent_id: Some(agent),
            query_text: "Rust".to_string(),
            memory_types: None,
            limit: 10,
            min_relevance: None,
            since: None,
            episode_id: None,
        };

        let results = store.search(&query).await.unwrap();
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| r.content.contains("Rust")));
    }

    #[tokio::test]
    async fn search_filters_by_memory_type() {
        let store = SqliteMemoryStore::open_in_memory().unwrap();
        let agent = make_agent_id();

        let e1 = make_entry(agent, "Rust is fast", MemoryType::Semantic);
        let e2 = make_entry(agent, "Rust compile step", MemoryType::Procedural);

        store.store_memory(&e1).await.unwrap();
        store.store_memory(&e2).await.unwrap();

        let query = MemoryQuery {
            agent_id: Some(agent),
            query_text: "Rust".to_string(),
            memory_types: Some(vec![MemoryType::Semantic]),
            limit: 10,
            min_relevance: None,
            since: None,
            episode_id: None,
        };

        let results = store.search(&query).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].memory_type, MemoryType::Semantic);
    }

    #[tokio::test]
    async fn episode_lifecycle() {
        let store = SqliteMemoryStore::open_in_memory().unwrap();
        let agent = make_agent_id();

        let episode = Episode {
            id: EpisodeId::new(),
            agent_id: agent,
            title: "Test conversation".to_string(),
            summary: None,
            started_at: Utc::now(),
            ended_at: None,
            turn_count: 0,
            metadata: serde_json::json!({}),
        };
        let ep_id = episode.id;

        store.create_episode(&episode).await.unwrap();

        let retrieved = store.get_episode(ep_id).await.unwrap();
        assert_eq!(retrieved.title, "Test conversation");
        assert!(retrieved.ended_at.is_none());

        store.end_episode(ep_id, "Discussed testing").await.unwrap();

        let ended = store.get_episode(ep_id).await.unwrap();
        assert_eq!(ended.summary.as_deref(), Some("Discussed testing"));
        assert!(ended.ended_at.is_some());
    }

    #[tokio::test]
    async fn list_episodes_ordered_by_most_recent() {
        let store = SqliteMemoryStore::open_in_memory().unwrap();
        let agent = make_agent_id();

        for i in 0..5 {
            let episode = Episode {
                id: EpisodeId::new(),
                agent_id: agent,
                title: format!("Episode {i}"),
                summary: None,
                started_at: Utc::now(),
                ended_at: None,
                turn_count: 0,
                metadata: serde_json::json!({}),
            };
            store.create_episode(&episode).await.unwrap();
        }

        let episodes = store.list_episodes(agent, 3).await.unwrap();
        assert_eq!(episodes.len(), 3);
    }

    #[tokio::test]
    async fn store_memory_with_provenance() {
        let store = SqliteMemoryStore::open_in_memory().unwrap();
        let agent = make_agent_id();
        let now = Utc::now();

        let entry = MemoryEntry {
            id: MemoryId::new(),
            agent_id: agent,
            episode_id: None,
            memory_type: MemoryType::Semantic,
            content: "provenance test".to_string(),
            embedding: None,
            metadata: serde_json::json!({}),
            provenance: Some(MemoryProvenance {
                source_run_id: Some("run-42".into()),
                source_turn: Some(3),
                extraction_method: "llm_extraction".into(),
                confidence: 0.9,
                verified: false,
                source_artifact_id: None,
                source_agent_id: None,
                ingested_at: None,
            }),
            created_at: now,
            accessed_at: now,
            access_count: 0,
            relevance_score: 1.0,
            confidence: 0.9,
            classification: Classification::default(),
        };
        let id = entry.id;

        store.store_memory(&entry).await.unwrap();
        let retrieved = store.get_memory(id).await.unwrap();

        let prov = retrieved.provenance.unwrap();
        assert_eq!(prov.source_run_id.as_deref(), Some("run-42"));
        assert_eq!(prov.source_turn, Some(3));
        assert_eq!(prov.extraction_method, "llm_extraction");
        assert!((prov.confidence - 0.9).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn store_memory_with_embedding() {
        let store = SqliteMemoryStore::open_in_memory().unwrap();
        let agent = make_agent_id();
        let now = Utc::now();

        let embedding = vec![1.0_f32, 2.0, 3.0, 4.0];
        let entry = MemoryEntry {
            id: MemoryId::new(),
            agent_id: agent,
            episode_id: None,
            memory_type: MemoryType::Semantic,
            content: "embedding test".to_string(),
            embedding: Some(embedding.clone()),
            metadata: serde_json::json!({}),
            provenance: None,
            created_at: now,
            accessed_at: now,
            access_count: 0,
            relevance_score: 1.0,
            confidence: 1.0,
            classification: Classification::default(),
        };
        let id = entry.id;

        store.store_memory(&entry).await.unwrap();
        let retrieved = store.get_memory(id).await.unwrap();

        let stored_embedding = retrieved.embedding.unwrap();
        assert_eq!(stored_embedding.len(), 4);
        for (a, b) in stored_embedding.iter().zip(embedding.iter()) {
            assert!((a - b).abs() < f32::EPSILON);
        }
    }

    #[tokio::test]
    async fn file_backed_store_with_tempfile() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("memory.db");

        let store = SqliteMemoryStore::open(&db_path).unwrap();
        let agent = make_agent_id();
        let entry = make_entry(agent, "persistent memory", MemoryType::Semantic);
        let id = entry.id;

        store.store_memory(&entry).await.unwrap();

        // Re-open the store from the same file.
        let store2 = SqliteMemoryStore::open(&db_path).unwrap();
        let retrieved = store2.get_memory(id).await.unwrap();
        assert_eq!(retrieved.content, "persistent memory");
    }

    #[tokio::test]
    async fn count_entries_returns_correct_count() {
        let store = SqliteMemoryStore::open_in_memory().unwrap();
        let agent = make_agent_id();

        assert_eq!(store.count_entries(&agent).await.unwrap(), 0);

        store
            .store_memory(&make_entry(agent, "one", MemoryType::Semantic))
            .await
            .unwrap();
        store
            .store_memory(&make_entry(agent, "two", MemoryType::Semantic))
            .await
            .unwrap();
        store
            .store_memory(&make_entry(agent, "three", MemoryType::Semantic))
            .await
            .unwrap();

        assert_eq!(store.count_entries(&agent).await.unwrap(), 3);

        let other = make_agent_id();
        assert_eq!(store.count_entries(&other).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn delete_by_age_removes_old_entries() {
        let store = SqliteMemoryStore::open_in_memory().unwrap();
        let agent = make_agent_id();
        let now = Utc::now();

        // Insert an old entry (3 days ago).
        let old = MemoryEntry {
            id: MemoryId::new(),
            agent_id: agent,
            episode_id: None,
            memory_type: MemoryType::Semantic,
            content: "old memory".to_string(),
            embedding: None,
            metadata: serde_json::json!({}),
            provenance: None,
            created_at: now - chrono::Duration::days(3),
            accessed_at: now,
            access_count: 0,
            relevance_score: 1.0,
            confidence: 1.0,
            classification: Classification::default(),
        };

        let recent = make_entry(agent, "recent memory", MemoryType::Semantic);

        store.store_memory(&old).await.unwrap();
        store.store_memory(&recent).await.unwrap();

        // Delete entries older than 1 day.
        let deleted = store
            .delete_by_age(&agent, chrono::Duration::days(1))
            .await
            .unwrap();

        assert_eq!(deleted, 1);
        assert_eq!(store.count_entries(&agent).await.unwrap(), 1);
    }

    #[tokio::test]
    async fn list_entries_returns_all_for_agent() {
        let store = SqliteMemoryStore::open_in_memory().unwrap();
        let agent = make_agent_id();

        for i in 0..5 {
            store
                .store_memory(&make_entry(
                    agent,
                    &format!("entry {i}"),
                    MemoryType::Semantic,
                ))
                .await
                .unwrap();
        }

        let entries = store.list_entries(&agent, 100, 0).await.unwrap();
        assert_eq!(entries.len(), 5);
    }

    #[tokio::test]
    async fn list_entries_pagination_limit() {
        let store = SqliteMemoryStore::open_in_memory().unwrap();
        let agent = make_agent_id();

        for i in 0..10 {
            store
                .store_memory(&make_entry(
                    agent,
                    &format!("entry {i}"),
                    MemoryType::Semantic,
                ))
                .await
                .unwrap();
        }

        let page1 = store.list_entries(&agent, 4, 0).await.unwrap();
        assert_eq!(page1.len(), 4);

        let page2 = store.list_entries(&agent, 4, 4).await.unwrap();
        assert_eq!(page2.len(), 4);

        let page3 = store.list_entries(&agent, 4, 8).await.unwrap();
        assert_eq!(page3.len(), 2);
    }

    #[tokio::test]
    async fn list_entries_pagination_non_overlapping() {
        let store = SqliteMemoryStore::open_in_memory().unwrap();
        let agent = make_agent_id();

        for i in 0..6 {
            store
                .store_memory(&make_entry(
                    agent,
                    &format!("entry {i}"),
                    MemoryType::Semantic,
                ))
                .await
                .unwrap();
        }

        let page1 = store.list_entries(&agent, 3, 0).await.unwrap();
        let page2 = store.list_entries(&agent, 3, 3).await.unwrap();

        let ids1: std::collections::HashSet<_> = page1.iter().map(|e| e.id).collect();
        let ids2: std::collections::HashSet<_> = page2.iter().map(|e| e.id).collect();

        // No overlap between pages.
        assert!(ids1.is_disjoint(&ids2));
        // Together they cover all 6 entries.
        assert_eq!(ids1.len() + ids2.len(), 6);
    }

    #[tokio::test]
    async fn list_entries_ordered_by_created_at_asc() {
        let store = SqliteMemoryStore::open_in_memory().unwrap();
        let agent = make_agent_id();
        let now = Utc::now();

        // Insert entries with descending timestamps so we can verify ordering.
        for i in (0..5_i64).rev() {
            let entry = MemoryEntry {
                id: MemoryId::new(),
                agent_id: agent,
                episode_id: None,
                memory_type: MemoryType::Semantic,
                content: format!("entry at t-{i}"),
                embedding: None,
                metadata: serde_json::json!({}),
                provenance: None,
                created_at: now - chrono::Duration::seconds(i * 10),
                accessed_at: now,
                access_count: 0,
                relevance_score: 1.0,
                confidence: 1.0,
                classification: Classification::default(),
            };
            store.store_memory(&entry).await.unwrap();
        }

        let entries = store.list_entries(&agent, 10, 0).await.unwrap();
        assert_eq!(entries.len(), 5);

        // Verify ascending order.
        for window in entries.windows(2) {
            assert!(
                window[0].created_at <= window[1].created_at,
                "list_entries should return entries ordered by created_at ASC"
            );
        }
    }

    #[tokio::test]
    async fn list_entries_excludes_other_agents() {
        let store = SqliteMemoryStore::open_in_memory().unwrap();
        let agent_a = make_agent_id();
        let agent_b = make_agent_id();

        store
            .store_memory(&make_entry(agent_a, "agent A entry", MemoryType::Semantic))
            .await
            .unwrap();
        store
            .store_memory(&make_entry(agent_b, "agent B entry", MemoryType::Semantic))
            .await
            .unwrap();

        let a_entries = store.list_entries(&agent_a, 10, 0).await.unwrap();
        assert_eq!(a_entries.len(), 1);
        assert_eq!(a_entries[0].content, "agent A entry");

        let b_entries = store.list_entries(&agent_b, 10, 0).await.unwrap();
        assert_eq!(b_entries.len(), 1);
        assert_eq!(b_entries[0].content, "agent B entry");
    }

    #[tokio::test]
    async fn list_entries_empty_store_returns_empty() {
        let store = SqliteMemoryStore::open_in_memory().unwrap();
        let agent = make_agent_id();

        let entries = store.list_entries(&agent, 10, 0).await.unwrap();
        assert!(entries.is_empty());
    }

    // -----------------------------------------------------------------------
    // Provenance tracking tests (PRD-09 §4.8)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn provenance_source_artifact_id_round_trip() {
        let store = SqliteMemoryStore::open_in_memory().unwrap();
        let agent = make_agent_id();
        let now = Utc::now();

        let entry = MemoryEntry {
            provenance: Some(MemoryProvenance {
                source_run_id: Some("run-100".into()),
                source_turn: Some(1),
                extraction_method: "tool_output".into(),
                confidence: 0.8,
                verified: false,
                source_artifact_id: Some("artifact-abc-123".into()),
                source_agent_id: Some("agent-xyz".into()),
                ingested_at: Some(now),
            }),
            ..make_entry(agent, "provenance artifact test", MemoryType::Semantic)
        };
        let id = entry.id;

        store.store_memory(&entry).await.unwrap();
        let retrieved = store.get_memory(id).await.unwrap();

        let prov = retrieved.provenance.unwrap();
        assert_eq!(prov.source_artifact_id.as_deref(), Some("artifact-abc-123"));
        assert_eq!(prov.source_agent_id.as_deref(), Some("agent-xyz"));
        assert!(prov.ingested_at.is_some());
    }

    #[tokio::test]
    async fn provenance_fields_default_to_none() {
        let store = SqliteMemoryStore::open_in_memory().unwrap();
        let agent = make_agent_id();

        let entry = MemoryEntry {
            provenance: Some(MemoryProvenance {
                source_run_id: None,
                source_turn: None,
                extraction_method: "user_input".into(),
                confidence: 1.0,
                verified: true,
                source_artifact_id: None,
                source_agent_id: None,
                ingested_at: None,
            }),
            ..make_entry(agent, "no artifact provenance", MemoryType::Semantic)
        };
        let id = entry.id;

        store.store_memory(&entry).await.unwrap();
        let retrieved = store.get_memory(id).await.unwrap();

        let prov = retrieved.provenance.unwrap();
        assert!(prov.source_artifact_id.is_none());
        assert!(prov.source_agent_id.is_none());
        assert!(prov.ingested_at.is_none());
    }

    #[tokio::test]
    async fn forget_removes_memories_by_artifact_id() {
        let store = SqliteMemoryStore::open_in_memory().unwrap();
        let agent = make_agent_id();

        // Store 2 memories from artifact-A, 1 from artifact-B.
        for i in 0..2 {
            let entry = MemoryEntry {
                provenance: Some(MemoryProvenance {
                    source_run_id: None,
                    source_turn: None,
                    extraction_method: "ingest".into(),
                    confidence: 1.0,
                    verified: false,
                    source_artifact_id: Some("artifact-A".into()),
                    source_agent_id: None,
                    ingested_at: None,
                }),
                ..make_entry(agent, &format!("from A #{i}"), MemoryType::Semantic)
            };
            store.store_memory(&entry).await.unwrap();
        }

        let entry_b = MemoryEntry {
            provenance: Some(MemoryProvenance {
                source_run_id: None,
                source_turn: None,
                extraction_method: "ingest".into(),
                confidence: 1.0,
                verified: false,
                source_artifact_id: Some("artifact-B".into()),
                source_agent_id: None,
                ingested_at: None,
            }),
            ..make_entry(agent, "from B", MemoryType::Semantic)
        };
        store.store_memory(&entry_b).await.unwrap();

        assert_eq!(store.count_entries(&agent).await.unwrap(), 3);

        let deleted = store.forget("artifact-A").await.unwrap();
        assert_eq!(deleted, 2);
        assert_eq!(store.count_entries(&agent).await.unwrap(), 1);
    }

    #[tokio::test]
    async fn forget_nonexistent_artifact_returns_zero() {
        let store = SqliteMemoryStore::open_in_memory().unwrap();
        let deleted = store.forget("no-such-artifact").await.unwrap();
        assert_eq!(deleted, 0);
    }

    #[tokio::test]
    async fn forget_removes_fts5_entries() {
        let store = SqliteMemoryStore::open_in_memory().unwrap();
        let agent = make_agent_id();

        let entry = MemoryEntry {
            provenance: Some(MemoryProvenance {
                source_run_id: None,
                source_turn: None,
                extraction_method: "ingest".into(),
                confidence: 1.0,
                verified: false,
                source_artifact_id: Some("fts-artifact".into()),
                source_agent_id: None,
                ingested_at: None,
            }),
            ..make_entry(agent, "unique_fts_token_zxyqw", MemoryType::Semantic)
        };
        store.store_memory(&entry).await.unwrap();

        // Verify searchable before forget.
        let query = MemoryQuery {
            agent_id: Some(agent),
            query_text: "unique_fts_token_zxyqw".to_string(),
            memory_types: None,
            limit: 10,
            min_relevance: None,
            since: None,
            episode_id: None,
        };
        let before = store.search(&query).await.unwrap();
        assert_eq!(before.len(), 1);

        store.forget("fts-artifact").await.unwrap();

        // Must not be searchable after forget.
        let after = store.search(&query).await.unwrap();
        assert!(
            after.is_empty(),
            "FTS5 index should be cleaned up after forget"
        );
    }

    #[tokio::test]
    async fn provenance_table_populated_with_new_fields() {
        let store = SqliteMemoryStore::open_in_memory().unwrap();
        let agent = make_agent_id();
        let now = Utc::now();

        let entry = MemoryEntry {
            provenance: Some(MemoryProvenance {
                source_run_id: Some("run-prov".into()),
                source_turn: Some(7),
                extraction_method: "llm_extraction".into(),
                confidence: 0.75,
                verified: true,
                source_artifact_id: Some("art-999".into()),
                source_agent_id: Some("agent-007".into()),
                ingested_at: Some(now),
            }),
            ..make_entry(
                agent,
                "check provenance table directly",
                MemoryType::Procedural,
            )
        };
        let id = entry.id;

        store.store_memory(&entry).await.unwrap();

        // Query the provenance table directly to verify new columns.
        let conn = store.inner.conn.lock();
        let (art_id, agent_id_col, ingested): (Option<String>, Option<String>, Option<String>) =
            conn.query_row(
                "SELECT source_artifact_id, source_agent_id, ingested_at
                 FROM memory_provenance WHERE memory_id = ?1",
                params![id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();

        assert_eq!(art_id.as_deref(), Some("art-999"));
        assert_eq!(agent_id_col.as_deref(), Some("agent-007"));
        assert!(ingested.is_some());
    }

    #[tokio::test]
    async fn search_with_classification_filters_restricted() {
        let store = SqliteMemoryStore::open_in_memory().unwrap();
        let agent = make_agent_id();
        let now = Utc::now();

        let public_entry = MemoryEntry {
            classification: Classification::Public,
            content: "public information about Rust".to_string(),
            ..make_entry(agent, "", MemoryType::Semantic)
        };
        // Fix the created_at so the entry is valid.
        let public_entry = MemoryEntry {
            created_at: now,
            accessed_at: now,
            ..public_entry
        };

        let restricted_entry = MemoryEntry {
            classification: Classification::Restricted,
            content: "restricted information about Rust".to_string(),
            ..make_entry(agent, "", MemoryType::Semantic)
        };
        let restricted_entry = MemoryEntry {
            created_at: now,
            accessed_at: now,
            ..restricted_entry
        };

        store.store_memory(&public_entry).await.unwrap();
        store.store_memory(&restricted_entry).await.unwrap();

        let query = MemoryQuery {
            agent_id: Some(agent),
            query_text: "Rust".to_string(),
            memory_types: None,
            limit: 10,
            min_relevance: None,
            since: None,
            episode_id: None,
        };

        let results = store
            .search_with_classification(&query, Classification::Internal)
            .await
            .unwrap();

        // Only Public should be visible under an Internal clearance.
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].classification, Classification::Public);
    }
}
