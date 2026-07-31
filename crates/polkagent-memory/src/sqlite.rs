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
    verified            INTEGER NOT NULL DEFAULT 0
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

        let embedding_bytes: Option<Vec<u8>> = entry.embedding.as_ref().map(|v| {
            v.iter()
                .flat_map(|f| f.to_le_bytes())
                .collect()
        });

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
                     (memory_id, source_run_id, source_turn, extraction_method, confidence, verified)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    entry.id.to_string(),
                    prov.source_run_id,
                    prov.source_turn,
                    prov.extraction_method,
                    prov.confidence,
                    i32::from(prov.verified),
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
        let agent_id: AgentId = agent_str
            .parse()
            .map_err(MemoryError::InvalidId)?;
        let episode_id = episode_str
            .map(|s| s.parse::<EpisodeId>())
            .transpose()?;
        let memory_type: MemoryType = type_str
            .parse()
            .map_err(MemoryError::InvalidOperation)?;

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
        let agent_id: AgentId = agent_str
            .parse()
            .map_err(MemoryError::InvalidId)?;
        let started_at = parse_timestamp(&started_str)?;
        let ended_at = ended_str
            .map(|s| parse_timestamp(&s))
            .transpose()?;
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
         WHERE fts.memories_fts MATCH ?1
           AND m.agent_id = ?2",
    );
    let mut param_idx = 3u32;

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
            let _ = write!(sql, " AND m.classification IN ({})", placeholders.join(", "));
        }
    }

    sql.push_str(" ORDER BY m.relevance_score DESC, rank LIMIT ?100");

    // Build a dynamic rusqlite params vector.
    let mut values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    values.push(Box::new(query.query_text.clone()));
    values.push(Box::new(query.agent_id.to_string()));
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
         WHERE agent_id = ?1",
    );
    let mut param_idx = 2u32;
    let mut extra_params: Vec<String> = Vec::new();

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
    values.push(Box::new(query.agent_id.to_string()));
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

        let e1 = make_entry(agent, "Rust is a systems programming language", MemoryType::Semantic);
        let e2 = make_entry(agent, "Python is great for scripting", MemoryType::Semantic);
        let e3 = make_entry(agent, "Rust has zero-cost abstractions", MemoryType::Procedural);

        store.store_memory(&e1).await.unwrap();
        store.store_memory(&e2).await.unwrap();
        store.store_memory(&e3).await.unwrap();

        let query = MemoryQuery {
            agent_id: agent,
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
            agent_id: agent,
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

        store.store_memory(&make_entry(agent, "one", MemoryType::Semantic)).await.unwrap();
        store.store_memory(&make_entry(agent, "two", MemoryType::Semantic)).await.unwrap();
        store.store_memory(&make_entry(agent, "three", MemoryType::Semantic)).await.unwrap();

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
        let public_entry = MemoryEntry { created_at: now, accessed_at: now, ..public_entry };

        let restricted_entry = MemoryEntry {
            classification: Classification::Restricted,
            content: "restricted information about Rust".to_string(),
            ..make_entry(agent, "", MemoryType::Semantic)
        };
        let restricted_entry =
            MemoryEntry { created_at: now, accessed_at: now, ..restricted_entry };

        store.store_memory(&public_entry).await.unwrap();
        store.store_memory(&restricted_entry).await.unwrap();

        let query = MemoryQuery {
            agent_id: agent,
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
