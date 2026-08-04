//! Core memory types: identifiers, entries, queries, episodes, and provenance.

use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use polkagent_core::ids::AgentId;

use crate::classification::Classification;

// ---------------------------------------------------------------------------
// MemoryId
// ---------------------------------------------------------------------------

/// Unique identifier for a memory entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MemoryId(Uuid);

impl MemoryId {
    /// Generate a new time-ordered (v7) identifier.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    /// Wrap an existing [`Uuid`] value.
    #[must_use]
    pub fn from_uuid(uuid: Uuid) -> Self {
        Self(uuid)
    }

    /// Return the inner [`Uuid`].
    #[must_use]
    pub fn as_uuid(&self) -> Uuid {
        self.0
    }
}

impl Default for MemoryId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for MemoryId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for MemoryId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(s).map(Self)
    }
}

impl From<Uuid> for MemoryId {
    fn from(uuid: Uuid) -> Self {
        Self(uuid)
    }
}

impl From<MemoryId> for Uuid {
    fn from(id: MemoryId) -> Self {
        id.0
    }
}

// ---------------------------------------------------------------------------
// EpisodeId
// ---------------------------------------------------------------------------

/// Unique identifier for an episode (conversation session).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EpisodeId(Uuid);

impl EpisodeId {
    /// Generate a new time-ordered (v7) identifier.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    /// Wrap an existing [`Uuid`] value.
    #[must_use]
    pub fn from_uuid(uuid: Uuid) -> Self {
        Self(uuid)
    }

    /// Return the inner [`Uuid`].
    #[must_use]
    pub fn as_uuid(&self) -> Uuid {
        self.0
    }
}

impl Default for EpisodeId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for EpisodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for EpisodeId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(s).map(Self)
    }
}

impl From<Uuid> for EpisodeId {
    fn from(uuid: Uuid) -> Self {
        Self(uuid)
    }
}

impl From<EpisodeId> for Uuid {
    fn from(id: EpisodeId) -> Self {
        id.0
    }
}

// ---------------------------------------------------------------------------
// MemoryType
// ---------------------------------------------------------------------------

/// The classification of a memory entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryType {
    /// Conversation history and interaction records.
    Episodic,
    /// Facts, knowledge, and declarative information.
    Semantic,
    /// Learned patterns, procedures, and skills.
    Procedural,
}

impl fmt::Display for MemoryType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Episodic => write!(f, "episodic"),
            Self::Semantic => write!(f, "semantic"),
            Self::Procedural => write!(f, "procedural"),
        }
    }
}

impl FromStr for MemoryType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "episodic" => Ok(Self::Episodic),
            "semantic" => Ok(Self::Semantic),
            "procedural" => Ok(Self::Procedural),
            other => Err(format!("unknown memory type: {other}")),
        }
    }
}

// ---------------------------------------------------------------------------
// MemoryProvenance
// ---------------------------------------------------------------------------

/// Tracks the origin and trustworthiness of a memory entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryProvenance {
    /// The run that produced this memory.
    pub source_run_id: Option<String>,
    /// The turn number within the run.
    pub source_turn: Option<u32>,
    /// How the memory was extracted (e.g. `user_input`, `llm_extraction`, `tool_output`).
    pub extraction_method: String,
    /// Confidence score from 0.0 to 1.0.
    pub confidence: f64,
    /// Whether a human has verified this memory.
    pub verified: bool,
    /// The artifact that sourced this memory (for `forget(artifact_id)` cascading).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_artifact_id: Option<String>,
    /// The agent that originally produced this memory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_agent_id: Option<String>,
    /// Timestamp when this memory was ingested into the store.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ingested_at: Option<DateTime<Utc>>,
}

// ---------------------------------------------------------------------------
// MemoryEntry
// ---------------------------------------------------------------------------

/// A single memory record stored by an agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    /// Unique identifier for this memory.
    pub id: MemoryId,
    /// The agent that owns this memory.
    pub agent_id: AgentId,
    /// Optional episode this memory belongs to.
    pub episode_id: Option<EpisodeId>,
    /// Classification of the memory (episodic / semantic / procedural).
    pub memory_type: MemoryType,
    /// The textual content of the memory.
    pub content: String,
    /// Optional embedding vector for future vector search support.
    pub embedding: Option<Vec<f32>>,
    /// Arbitrary key-value metadata.
    pub metadata: serde_json::Value,
    /// Provenance tracking for this memory.
    pub provenance: Option<MemoryProvenance>,
    /// When the memory was created.
    pub created_at: DateTime<Utc>,
    /// When the memory was last accessed.
    pub accessed_at: DateTime<Utc>,
    /// Number of times this memory has been retrieved.
    pub access_count: u64,
    /// Relevance score (higher = more relevant, decays over time).
    pub relevance_score: f64,
    /// Confidence in the accuracy of this memory (0.0 – 1.0).
    ///
    /// Defaults to `1.0` (fully confident). Used by admission control to
    /// reject low-quality memories before they reach the store.
    #[serde(default = "default_confidence")]
    pub confidence: f64,
    /// Security / sensitivity classification for this memory.
    ///
    /// Defaults to [`Classification::Internal`]. Used by [`ClassificationFilter`]
    /// at query time to restrict results to the caller's clearance level.
    ///
    /// [`ClassificationFilter`]: crate::classification::ClassificationFilter
    #[serde(default)]
    pub classification: Classification,
}

fn default_confidence() -> f64 {
    1.0
}

// ---------------------------------------------------------------------------
// MemoryQuery
// ---------------------------------------------------------------------------

/// Parameters for searching memories.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryQuery {
    /// The agent whose memories to search. `None` searches across all agents.
    pub agent_id: Option<AgentId>,
    /// Free-text query string.
    pub query_text: String,
    /// Optional filter to specific memory types.
    pub memory_types: Option<Vec<MemoryType>>,
    /// Maximum number of results to return.
    pub limit: usize,
    /// Minimum relevance score threshold.
    pub min_relevance: Option<f64>,
    /// Only return memories created after this timestamp.
    pub since: Option<DateTime<Utc>>,
    /// Only return memories from this episode.
    pub episode_id: Option<EpisodeId>,
}

// ---------------------------------------------------------------------------
// Episode
// ---------------------------------------------------------------------------

/// A conversation session or interaction episode.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Episode {
    /// Unique identifier for this episode.
    pub id: EpisodeId,
    /// The agent that participated in this episode.
    pub agent_id: AgentId,
    /// Human-readable title for the episode.
    pub title: String,
    /// Optional summary generated after the episode ends.
    pub summary: Option<String>,
    /// When the episode started.
    pub started_at: DateTime<Utc>,
    /// When the episode ended (None if still active).
    pub ended_at: Option<DateTime<Utc>>,
    /// Number of turns in the episode.
    pub turn_count: u32,
    /// Arbitrary key-value metadata.
    pub metadata: serde_json::Value,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_id_new_is_unique() {
        let a = MemoryId::new();
        let b = MemoryId::new();
        assert_ne!(a, b);
    }

    #[test]
    fn memory_id_display_and_from_str_round_trip() {
        let id = MemoryId::new();
        let s = id.to_string();
        let parsed: MemoryId = s.parse().unwrap();
        assert_eq!(id, parsed);
    }

    #[test]
    fn memory_id_serde_round_trip() {
        let id = MemoryId::new();
        let json = serde_json::to_string(&id).unwrap();
        assert!(json.starts_with('"'));
        let back: MemoryId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, back);
    }

    #[test]
    fn episode_id_new_is_unique() {
        let a = EpisodeId::new();
        let b = EpisodeId::new();
        assert_ne!(a, b);
    }

    #[test]
    fn episode_id_serde_round_trip() {
        let id = EpisodeId::new();
        let json = serde_json::to_string(&id).unwrap();
        let back: EpisodeId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, back);
    }

    #[test]
    fn memory_type_display() {
        assert_eq!(MemoryType::Episodic.to_string(), "episodic");
        assert_eq!(MemoryType::Semantic.to_string(), "semantic");
        assert_eq!(MemoryType::Procedural.to_string(), "procedural");
    }

    #[test]
    fn memory_type_from_str() {
        assert_eq!(
            "episodic".parse::<MemoryType>().unwrap(),
            MemoryType::Episodic
        );
        assert_eq!(
            "semantic".parse::<MemoryType>().unwrap(),
            MemoryType::Semantic
        );
        assert_eq!(
            "procedural".parse::<MemoryType>().unwrap(),
            MemoryType::Procedural
        );
        assert!("unknown".parse::<MemoryType>().is_err());
    }

    #[test]
    fn memory_type_serde_round_trip() {
        let t = MemoryType::Semantic;
        let json = serde_json::to_string(&t).unwrap();
        assert_eq!(json, r#""semantic""#);
        let back: MemoryType = serde_json::from_str(&json).unwrap();
        assert_eq!(back, t);
    }

    #[test]
    fn memory_provenance_serde_round_trip() {
        let p = MemoryProvenance {
            source_run_id: Some("run-123".into()),
            source_turn: Some(5),
            extraction_method: "llm_extraction".into(),
            confidence: 0.95,
            verified: false,
            source_artifact_id: Some("artifact-abc".into()),
            source_agent_id: Some("agent-xyz".into()),
            ingested_at: None,
        };
        let json = serde_json::to_string(&p).unwrap();
        let back: MemoryProvenance = serde_json::from_str(&json).unwrap();
        assert_eq!(back.confidence, 0.95);
        assert_eq!(back.extraction_method, "llm_extraction");
    }
}
