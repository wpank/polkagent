//! `polkagent-memory` — Episodic, semantic, and procedural memory for agents.
//!
//! This crate provides a memory subsystem for Polkagent agents with three
//! memory types and full provenance tracking:
//!
//! - **Episodic** — conversation history and interaction records
//! - **Semantic** — facts, knowledge, and declarative information
//! - **Procedural** — learned patterns, procedures, and skills
//!
//! # Architecture
//!
//! | Layer | Module | Purpose |
//! |-------|--------|---------|
//! | Types | [`types`] | Core data structures: entries, queries, episodes |
//! | Trait | [`store`] | Abstract [`MemoryStore`] trait |
//! | `SQLite` | [`sqlite`] | Concrete implementation with FTS5 full-text search |
//! | Facade | [`service`] | High-level [`MemoryService`] API |
//! | Errors | [`error`] | [`MemoryError`] enum |
//!
//! # Quickstart
//!
//! ```no_run
//! use std::sync::Arc;
//! use polkagent_core::ids::AgentId;
//! use polkagent_memory::sqlite::SqliteMemoryStore;
//! use polkagent_memory::service::MemoryService;
//! use polkagent_memory::types::MemoryType;
//!
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! let store = SqliteMemoryStore::open("memory.db")?;
//! let svc = MemoryService::new(Arc::new(store));
//!
//! let agent = AgentId::new();
//! let id = svc.remember(agent, "The user prefers dark mode", MemoryType::Semantic, None).await?;
//! let results = svc.recall(agent, "dark mode", 5).await?;
//! # Ok(())
//! # }
//! ```

#![forbid(unsafe_code)]
#![warn(
    missing_docs,
    clippy::pedantic,
    clippy::unwrap_used,
    clippy::expect_used
)]
#![cfg_attr(
    test,
    allow(
        clippy::expect_used,
        clippy::float_cmp,
        clippy::panic,
        clippy::unwrap_used,
        reason = "unit tests intentionally assert fixture and persistence invariants with immediate failures"
    )
)]
#![allow(
    clippy::module_name_repetitions,
    clippy::must_use_candidate,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc
)]

pub mod admission;
pub mod classification;
pub mod conformance;
pub mod embedding;
pub mod episode;
pub mod error;
pub mod export;
pub mod metadata_rag;
pub mod retention;
pub mod retrieval;
pub mod service;
pub mod sqlite;
pub mod store;
pub mod tenant;
pub mod types;

// ---------------------------------------------------------------------------
// Convenience re-exports
// ---------------------------------------------------------------------------

pub use classification::Classification;
pub use embedding::{
    EmbeddingProvider, EmbeddingVector, MockEmbeddingProvider, SearchResult, SimilarityMetric,
    VectorIndex,
};
pub use episode::{
    EpisodeEntry, EpisodeLogger, EpisodeLoggerConfig, RedactionRule, Role, StandardRedactor,
    ToolCallRecord,
};
pub use error::{MemoryError, MemoryResult};
pub use export::ImportResult;
pub use metadata_rag::{CitedResult, MetadataCitation, MetadataDocument, MetadataRagService};
pub use retention::{RetentionPolicy, RetentionSweeper, SweepResult};
pub use retrieval::{
    ContextBudget, HybridRetriever, InMemoryVectorIndex, RankedResult, RetrievalConfig,
    SqliteVecIndex, VectorSearchIndex,
};
pub use service::MemoryService;
pub use sqlite::SqliteMemoryStore;
pub use store::MemoryStore;
pub use types::{
    Episode, EpisodeId, MemoryEntry, MemoryId, MemoryProvenance, MemoryQuery, MemoryType,
};
