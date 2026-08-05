//! Chain knowledge query tool (PRD-05 workflow A9).
//!
//! The [`ChainKnowledgeTool`] lets agents query runtime metadata that has been
//! ingested into the FTS5-backed memory store.  Results are returned with
//! provenance citations containing the metadata hash, block number, pallet
//! name, and source section.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tracing::debug;

use polkagent_core::config::DataClassification;
use polkagent_memory::metadata_rag::MetadataRagService;
use polkagent_memory::store::MemoryStore;

use crate::registry::{ToolContext, ToolError, ToolHandler, ToolResult, ToolSpec};

// ---------------------------------------------------------------------------
// ChainKnowledgeTool
// ---------------------------------------------------------------------------

/// Queries chain knowledge grounded in runtime metadata.
///
/// Delegates to [`MetadataRagService`] for FTS5 search and citation
/// reconstruction.  Requires a [`MemoryStore`] that has been populated with
/// metadata documents via [`MetadataRagService::ingest`].
pub struct ChainKnowledgeTool {
    store: Arc<dyn MemoryStore>,
}

impl ChainKnowledgeTool {
    /// Create a new `ChainKnowledgeTool` backed by the given store.
    pub fn new(store: Arc<dyn MemoryStore>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl ToolHandler for ChainKnowledgeTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "polkagent.chain.knowledge".to_string(),
            description: concat!(
                "Query chain knowledge grounded in runtime metadata. ",
                "Returns answers with provenance citations including metadata hash, ",
                "block number, pallet name, and source section."
            )
            .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Natural-language query about chain functionality (e.g. 'How do I transfer tokens?', 'What storage does Staking use?')"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of cited results to return (default 5)"
                    }
                },
                "required": ["query"]
            }),
            required_grant: Some("chain.query".to_string()),
            output_classification: DataClassification::Internal,
        }
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let query_text =
            input
                .get("query")
                .and_then(Value::as_str)
                .ok_or_else(|| ToolError::InvalidInput {
                    reason: "missing or invalid 'query' field".to_string(),
                })?;

        let limit = input.get("limit").and_then(Value::as_u64).unwrap_or(5) as usize;

        debug!(
            query = query_text,
            limit,
            agent = %context.agent_id,
            "querying chain knowledge"
        );

        let svc = MetadataRagService::new(self.store.as_ref(), context.agent_id);

        let cited_results =
            svc.query(query_text, limit)
                .await
                .map_err(|e| ToolError::ExecutionFailed {
                    reason: format!("chain knowledge query failed: {e}"),
                })?;

        let results: Vec<Value> = cited_results
            .iter()
            .map(|r| {
                serde_json::json!({
                    "content": r.content,
                    "relevance_score": r.relevance_score,
                    "citation": {
                        "metadata_hash": r.citation.metadata_hash,
                        "block_number": r.citation.block_number,
                        "pallet_name": r.citation.pallet_name,
                        "source_section": r.citation.source_section,
                    }
                })
            })
            .collect();

        Ok(ToolResult {
            output: serde_json::json!({
                "query": query_text,
                "count": results.len(),
                "results": results,
            }),
            classification: DataClassification::Internal,
            artifacts: vec![],
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use polkagent_memory::metadata_rag::{MetadataDocument, MetadataRagService};

    #[test]
    fn chain_knowledge_spec() {
        let store = Arc::new(FakeMemoryStore);
        let tool = ChainKnowledgeTool::new(store);
        let spec = tool.spec();

        assert_eq!(spec.name, "polkagent.chain.knowledge");
        assert_eq!(spec.required_grant.as_deref(), Some("chain.query"));
        assert_eq!(spec.output_classification, DataClassification::Internal);
    }

    #[tokio::test]
    async fn query_missing_field() {
        let store = Arc::new(FakeMemoryStore);
        let tool = ChainKnowledgeTool::new(store);

        let ctx = ToolContext {
            run_id: polkagent_core::RunId::new(),
            agent_id: polkagent_core::AgentId::new(),
            step_id: polkagent_core::StepId::new(),
            grants: vec![],
            security_config: None,
        };

        let result = tool.execute(serde_json::json!({}), &ctx).await;
        assert!(matches!(result, Err(ToolError::InvalidInput { .. })));
    }

    #[tokio::test]
    async fn query_returns_empty_on_empty_store() {
        let store = Arc::new(FakeMemoryStore);
        let tool = ChainKnowledgeTool::new(store);

        let ctx = ToolContext {
            run_id: polkagent_core::RunId::new(),
            agent_id: polkagent_core::AgentId::new(),
            step_id: polkagent_core::StepId::new(),
            grants: vec![],
            security_config: None,
        };

        let input = serde_json::json!({ "query": "transfer" });
        let result = tool.execute(input, &ctx).await.unwrap();
        assert_eq!(result.output["count"], 0);
    }

    #[tokio::test]
    async fn query_returns_cited_results() {
        let store = polkagent_memory::sqlite::SqliteMemoryStore::open_in_memory().unwrap();
        let store = Arc::new(store);
        let agent_id = polkagent_core::AgentId::new();

        // Ingest metadata documents.
        let svc = MetadataRagService::new(store.as_ref(), agent_id);
        let docs = vec![MetadataDocument {
            chain_id: "polkadot".into(),
            pallet_name: "Balances".into(),
            section: "call".into(),
            item_name: "transfer_allow_death".into(),
            content: "Pallet: Balances\nCall: transfer_allow_death\nSignature: Balances.transfer_allow_death(dest: AccountId, value: Balance)".into(),
            metadata_hash: "deadbeef".into(),
            block_number: 100,
        }];
        svc.ingest(&docs).await.unwrap();

        // Query via the tool.
        let tool = ChainKnowledgeTool::new(store);
        let ctx = ToolContext {
            run_id: polkagent_core::RunId::new(),
            agent_id,
            step_id: polkagent_core::StepId::new(),
            grants: vec![],
            security_config: None,
        };

        let input = serde_json::json!({ "query": "Balances transfer" });
        let result = tool.execute(input, &ctx).await.unwrap();

        assert_eq!(result.output["count"], 1);
        let first = &result.output["results"][0];
        assert_eq!(first["citation"]["pallet_name"], "Balances");
        assert_eq!(first["citation"]["metadata_hash"], "deadbeef");
        assert_eq!(first["citation"]["block_number"], 100);
        assert_eq!(first["citation"]["source_section"], "call");
    }

    /// Minimal fake memory store for spec-only tests.
    struct FakeMemoryStore;

    #[async_trait]
    impl MemoryStore for FakeMemoryStore {
        async fn store_memory(
            &self,
            _entry: &polkagent_memory::types::MemoryEntry,
        ) -> polkagent_memory::MemoryResult<polkagent_memory::types::MemoryId> {
            Ok(polkagent_memory::types::MemoryId::new())
        }

        async fn get_memory(
            &self,
            _id: polkagent_memory::types::MemoryId,
        ) -> polkagent_memory::MemoryResult<polkagent_memory::types::MemoryEntry> {
            Err(polkagent_memory::MemoryError::NotFound(
                "fake store".to_string(),
            ))
        }

        async fn search(
            &self,
            _query: &polkagent_memory::types::MemoryQuery,
        ) -> polkagent_memory::MemoryResult<Vec<polkagent_memory::types::MemoryEntry>> {
            Ok(vec![])
        }

        async fn update_relevance(
            &self,
            _id: polkagent_memory::types::MemoryId,
            _score: f64,
        ) -> polkagent_memory::MemoryResult<()> {
            Ok(())
        }

        async fn delete_memory(
            &self,
            _id: polkagent_memory::types::MemoryId,
        ) -> polkagent_memory::MemoryResult<()> {
            Ok(())
        }

        async fn create_episode(
            &self,
            _episode: &polkagent_memory::types::Episode,
        ) -> polkagent_memory::MemoryResult<polkagent_memory::types::EpisodeId> {
            Ok(polkagent_memory::types::EpisodeId::new())
        }

        async fn get_episode(
            &self,
            _id: polkagent_memory::types::EpisodeId,
        ) -> polkagent_memory::MemoryResult<polkagent_memory::types::Episode> {
            Err(polkagent_memory::MemoryError::NotFound(
                "fake store".to_string(),
            ))
        }

        async fn end_episode(
            &self,
            _id: polkagent_memory::types::EpisodeId,
            _summary: &str,
        ) -> polkagent_memory::MemoryResult<()> {
            Ok(())
        }

        async fn list_episodes(
            &self,
            _agent_id: polkagent_core::AgentId,
            _limit: usize,
        ) -> polkagent_memory::MemoryResult<Vec<polkagent_memory::types::Episode>> {
            Ok(vec![])
        }

        async fn search_with_classification(
            &self,
            _query: &polkagent_memory::types::MemoryQuery,
            _max_classification: polkagent_memory::Classification,
        ) -> polkagent_memory::MemoryResult<Vec<polkagent_memory::types::MemoryEntry>> {
            Ok(vec![])
        }

        async fn list_entries(
            &self,
            _agent_id: &polkagent_core::AgentId,
            _limit: usize,
            _offset: usize,
        ) -> polkagent_memory::MemoryResult<Vec<polkagent_memory::types::MemoryEntry>> {
            Ok(vec![])
        }

        async fn count_entries(
            &self,
            _agent_id: &polkagent_core::AgentId,
        ) -> polkagent_memory::MemoryResult<usize> {
            Ok(0)
        }

        async fn forget(&self, _artifact_id: &str) -> polkagent_memory::MemoryResult<usize> {
            Ok(0)
        }

        async fn delete_by_age(
            &self,
            _agent_id: &polkagent_core::AgentId,
            _max_age: chrono::TimeDelta,
        ) -> polkagent_memory::MemoryResult<usize> {
            Ok(0)
        }
    }
}
