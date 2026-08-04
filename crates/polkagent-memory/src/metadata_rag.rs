//! Metadata-grounded RAG for Polkadot chain knowledge (PRD-05 workflow A9).
//!
//! Ingests runtime metadata (pallet docs, call signatures, storage descriptions)
//! into the FTS5 index and returns query results with provenance citations.

use chrono::Utc;
use serde::{Deserialize, Serialize};
use tracing::debug;

use polkagent_core::ids::AgentId;

use crate::classification::Classification;
use crate::error::MemoryResult;
use crate::store::MemoryStore;
use crate::types::{MemoryEntry, MemoryId, MemoryProvenance, MemoryQuery, MemoryType};

// ---------------------------------------------------------------------------
// Citation
// ---------------------------------------------------------------------------

/// A provenance citation grounding an answer in on-chain metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetadataCitation {
    /// BLAKE3 hash of the runtime metadata blob.
    pub metadata_hash: String,
    /// Block number at which the metadata was fetched.
    pub block_number: u64,
    /// Name of the pallet this knowledge comes from.
    pub pallet_name: String,
    /// Section within the pallet (e.g. `"call"`, `"storage"`, `"constant"`, `"event"`).
    pub source_section: String,
}

// ---------------------------------------------------------------------------
// MetadataDocument
// ---------------------------------------------------------------------------

/// A single knowledge document extracted from runtime metadata, ready for
/// ingestion into the FTS5 index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetadataDocument {
    /// Chain identifier (e.g. `"polkadot"`, `"kusama"`).
    pub chain_id: String,
    /// Name of the pallet.
    pub pallet_name: String,
    /// Section within the pallet (`"call"`, `"storage"`, `"constant"`, `"event"`).
    pub section: String,
    /// Name of the specific item (call name, storage entry, constant name, etc.).
    pub item_name: String,
    /// Human-readable content describing this item (signature, docs, type info).
    pub content: String,
    /// BLAKE3 hash of the runtime metadata blob.
    pub metadata_hash: String,
    /// Block number at which the metadata was fetched.
    pub block_number: u64,
}

// ---------------------------------------------------------------------------
// CitedResult
// ---------------------------------------------------------------------------

/// A single search result with its provenance citation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CitedResult {
    /// The knowledge content.
    pub content: String,
    /// Provenance citation grounding this result in on-chain metadata.
    pub citation: MetadataCitation,
    /// Relevance score from the retrieval engine.
    pub relevance_score: f64,
}

// ---------------------------------------------------------------------------
// MetadataRagService
// ---------------------------------------------------------------------------

/// Service for metadata-grounded retrieval-augmented generation.
///
/// Ingests [`MetadataDocument`]s into the memory store as `Semantic` entries
/// with structured metadata for citation reconstruction, then supports
/// querying with full provenance citations.
pub struct MetadataRagService<'a> {
    store: &'a dyn MemoryStore,
    agent_id: AgentId,
}

impl<'a> MetadataRagService<'a> {
    /// Create a new service for the given agent and backing store.
    pub fn new(store: &'a dyn MemoryStore, agent_id: AgentId) -> Self {
        Self { store, agent_id }
    }

    /// Ingest a batch of metadata documents into the FTS5-backed memory store.
    ///
    /// Each document is stored as a `Semantic` memory entry. The citation
    /// fields are preserved in the entry's `metadata` JSON so they can be
    /// reconstructed at query time.
    ///
    /// Returns the number of documents successfully ingested.
    pub async fn ingest(&self, documents: &[MetadataDocument]) -> MemoryResult<usize> {
        let mut count = 0usize;
        let now = Utc::now();

        for doc in documents {
            let citation_meta = serde_json::json!({
                "chain_id": doc.chain_id,
                "pallet_name": doc.pallet_name,
                "section": doc.section,
                "item_name": doc.item_name,
                "metadata_hash": doc.metadata_hash,
                "block_number": doc.block_number,
                "source": "runtime_metadata",
            });

            let entry = MemoryEntry {
                id: MemoryId::new(),
                agent_id: self.agent_id,
                episode_id: None,
                memory_type: MemoryType::Semantic,
                content: doc.content.clone(),
                embedding: None,
                metadata: citation_meta,
                provenance: Some(MemoryProvenance {
                    source_run_id: None,
                    source_turn: None,
                    extraction_method: "runtime_metadata_ingest".to_string(),
                    confidence: 1.0,
                    verified: true,
                    source_artifact_id: None,
                    source_agent_id: None,
                    ingested_at: None,
                }),
                created_at: now,
                accessed_at: now,
                access_count: 0,
                relevance_score: 1.0,
                confidence: 1.0,
                classification: Classification::Internal,
            };

            self.store.store_memory(&entry).await?;
            count += 1;
        }

        debug!(
            agent_id = %self.agent_id,
            ingested = count,
            "ingested metadata documents"
        );

        Ok(count)
    }

    /// Query chain knowledge and return results with provenance citations.
    ///
    /// Searches the FTS5 index for entries matching `query_text` that were
    /// ingested from runtime metadata, then reconstructs a [`MetadataCitation`]
    /// for each result.
    pub async fn query(&self, query_text: &str, limit: usize) -> MemoryResult<Vec<CitedResult>> {
        let mem_query = MemoryQuery {
            agent_id: Some(self.agent_id),
            query_text: query_text.to_string(),
            memory_types: Some(vec![MemoryType::Semantic]),
            limit,
            min_relevance: None,
            since: None,
            episode_id: None,
        };

        let entries = self.store.search(&mem_query).await?;

        let results = entries
            .into_iter()
            .filter_map(|entry| {
                let meta = &entry.metadata;

                // Only include entries that were ingested from runtime metadata.
                if meta.get("source").and_then(|v| v.as_str()) != Some("runtime_metadata") {
                    return None;
                }

                let citation = MetadataCitation {
                    metadata_hash: meta
                        .get("metadata_hash")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    block_number: meta
                        .get("block_number")
                        .and_then(|v| v.as_u64())
                        .unwrap_or_default(),
                    pallet_name: meta
                        .get("pallet_name")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    source_section: meta
                        .get("section")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                };

                Some(CitedResult {
                    content: entry.content,
                    citation,
                    relevance_score: entry.relevance_score,
                })
            })
            .collect();

        Ok(results)
    }
}

// ---------------------------------------------------------------------------
// Helpers — build MetadataDocuments from parsed metadata
// ---------------------------------------------------------------------------

/// Build a human-readable content string for a pallet call.
///
/// `args` is a list of `(name, type_name)` pairs describing the call parameters.
#[must_use]
pub fn format_call_doc(pallet_name: &str, call_name: &str, args: &[(String, String)]) -> String {
    let mut s =
        format!("Pallet: {pallet_name}\nCall: {call_name}\nSignature: {pallet_name}.{call_name}(");
    for (i, (name, ty)) in args.iter().enumerate() {
        if i > 0 {
            s.push_str(", ");
        }
        s.push_str(&format!("{name}: {ty}"));
    }
    s.push(')');
    s
}

/// Build a human-readable content string for a pallet storage entry.
#[must_use]
pub fn format_storage_doc(pallet_name: &str, storage_prefix: &str) -> String {
    format!("Pallet: {pallet_name}\nStorage prefix: {storage_prefix}")
}

/// Build a human-readable content string for a pallet constant.
#[must_use]
pub fn format_constant_doc(pallet_name: &str, constant_name: &str, type_id: u32) -> String {
    format!("Pallet: {pallet_name}\nConstant: {constant_name}\nType ID: {type_id}")
}

/// Build a human-readable content string for a pallet event type.
#[must_use]
pub fn format_event_doc(pallet_name: &str, event_type_id: u32) -> String {
    format!("Pallet: {pallet_name}\nEvents type ID: {event_type_id}")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sqlite::SqliteMemoryStore;

    #[tokio::test]
    async fn ingest_and_query_round_trip() {
        let store = SqliteMemoryStore::open_in_memory().unwrap();
        let agent_id = AgentId::new();
        let svc = MetadataRagService::new(&store, agent_id);

        let docs = vec![
            MetadataDocument {
                chain_id: "polkadot".into(),
                pallet_name: "Balances".into(),
                section: "call".into(),
                item_name: "transfer_allow_death".into(),
                content: format_call_doc(
                    "Balances",
                    "transfer_allow_death",
                    &[
                        ("dest".into(), "AccountId".into()),
                        ("value".into(), "Balance".into()),
                    ],
                ),
                metadata_hash: "abc123".into(),
                block_number: 42,
            },
            MetadataDocument {
                chain_id: "polkadot".into(),
                pallet_name: "Staking".into(),
                section: "call".into(),
                item_name: "bond".into(),
                content: format_call_doc("Staking", "bond", &[("value".into(), "Balance".into())]),
                metadata_hash: "abc123".into(),
                block_number: 42,
            },
        ];

        let ingested = svc.ingest(&docs).await.unwrap();
        assert_eq!(ingested, 2);

        // Query for balances-related knowledge.
        let results = svc.query("Balances transfer", 10).await.unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].content.contains("transfer_allow_death"));
        assert_eq!(results[0].citation.pallet_name, "Balances");
        assert_eq!(results[0].citation.metadata_hash, "abc123");
        assert_eq!(results[0].citation.block_number, 42);
        assert_eq!(results[0].citation.source_section, "call");
    }

    #[tokio::test]
    async fn query_returns_empty_for_no_match() {
        let store = SqliteMemoryStore::open_in_memory().unwrap();
        let agent_id = AgentId::new();
        let svc = MetadataRagService::new(&store, agent_id);

        let results = svc.query("nonexistent pallet", 10).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn ingest_empty_batch_returns_zero() {
        let store = SqliteMemoryStore::open_in_memory().unwrap();
        let agent_id = AgentId::new();
        let svc = MetadataRagService::new(&store, agent_id);

        let ingested = svc.ingest(&[]).await.unwrap();
        assert_eq!(ingested, 0);
    }

    #[tokio::test]
    async fn non_metadata_entries_excluded_from_results() {
        let store = SqliteMemoryStore::open_in_memory().unwrap();
        let agent_id = AgentId::new();
        let svc = MetadataRagService::new(&store, agent_id);

        // Store a regular semantic memory (not from metadata ingest).
        let now = Utc::now();
        let entry = MemoryEntry {
            id: MemoryId::new(),
            agent_id,
            episode_id: None,
            memory_type: MemoryType::Semantic,
            content: "Balances pallet handles token transfers".into(),
            embedding: None,
            metadata: serde_json::json!({}),
            provenance: None,
            created_at: now,
            accessed_at: now,
            access_count: 0,
            relevance_score: 1.0,
            confidence: 1.0,
            classification: Classification::Internal,
        };
        store.store_memory(&entry).await.unwrap();

        // Query should NOT return the manually-stored entry.
        let results = svc.query("Balances", 10).await.unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn format_call_doc_includes_signature() {
        let doc = format_call_doc(
            "Balances",
            "transfer",
            &[
                ("dest".into(), "AccountId".into()),
                ("value".into(), "Balance".into()),
            ],
        );
        assert!(doc.contains("Balances.transfer(dest: AccountId, value: Balance)"));
    }

    #[test]
    fn format_storage_doc_content() {
        let doc = format_storage_doc("System", "Account");
        assert!(doc.contains("System"));
        assert!(doc.contains("Account"));
    }

    #[test]
    fn format_constant_doc_content() {
        let doc = format_constant_doc("Balances", "ExistentialDeposit", 6);
        assert!(doc.contains("ExistentialDeposit"));
        assert!(doc.contains("Type ID: 6"));
    }

    #[test]
    fn format_event_doc_content() {
        let doc = format_event_doc("Balances", 21);
        assert!(doc.contains("Balances"));
        assert!(doc.contains("Events type ID: 21"));
    }
}
