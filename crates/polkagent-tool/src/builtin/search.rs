//! Memory search tool.
//!
//! The [`SearchMemoryTool`] queries the agent's memory store for relevant
//! entries. It delegates to a [`MemoryStore`] implementation provided at
//! construction time.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tracing::debug;

use polkagent_core::config::DataClassification;
use polkagent_memory::store::MemoryStore;
use polkagent_memory::types::MemoryQuery;

use crate::registry::{ToolContext, ToolError, ToolHandler, ToolResult, ToolSpec};

// ---------------------------------------------------------------------------
// SearchMemoryTool
// ---------------------------------------------------------------------------

/// Searches agent memory for entries matching a free-text query.
///
/// Delegates to whatever [`MemoryStore`] implementation is injected at
/// construction time.
///
/// No grant is required for memory search — the tool already operates within
/// the agent's own memory scope.
pub struct SearchMemoryTool {
    store: Arc<dyn MemoryStore>,
}

impl SearchMemoryTool {
    /// Create a new `SearchMemoryTool` backed by the given store.
    pub fn new(store: Arc<dyn MemoryStore>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl ToolHandler for SearchMemoryTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "polkagent.memory.search".to_string(),
            description: "Search agent memory for relevant entries matching a query.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Free-text search query"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of results (default 10)"
                    },
                    "memory_type": {
                        "type": "string",
                        "enum": ["episodic", "semantic", "procedural"],
                        "description": "Optional filter to a specific memory type"
                    }
                },
                "required": ["query"]
            }),
            required_grant: None,
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

        let requested_limit = input.get("limit").and_then(Value::as_u64).unwrap_or(10);
        let limit = usize::try_from(requested_limit).map_err(|_| ToolError::InvalidInput {
            reason: format!("limit {requested_limit} exceeds the supported range"),
        })?;

        let memory_types = input
            .get("memory_type")
            .and_then(Value::as_str)
            .and_then(|s| s.parse::<polkagent_memory::types::MemoryType>().ok())
            .map(|t| vec![t]);

        debug!(
            query = query_text,
            limit,
            agent = %context.agent_id,
            "searching memory"
        );

        let query = MemoryQuery {
            agent_id: Some(context.agent_id),
            query_text: query_text.to_string(),
            memory_types,
            limit,
            min_relevance: None,
            since: None,
            episode_id: None,
        };

        let entries = self
            .store
            .search(&query)
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                reason: format!("memory search failed: {e}"),
            })?;

        let results: Vec<Value> = entries
            .iter()
            .map(|entry| {
                serde_json::json!({
                    "id": entry.id.to_string(),
                    "memory_type": entry.memory_type.to_string(),
                    "content": entry.content,
                    "relevance_score": entry.relevance_score,
                    "created_at": entry.created_at.to_rfc3339(),
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

    #[test]
    fn search_memory_spec() {
        // We need a store to construct the tool, but we only need the spec.
        // Use a fake in-memory store.
        let store = Arc::new(FakeMemoryStore);
        let tool = SearchMemoryTool::new(store);
        let spec = tool.spec();

        assert_eq!(spec.name, "polkagent.memory.search");
        assert!(spec.required_grant.is_none());
        assert_eq!(spec.output_classification, DataClassification::Internal);
    }

    #[tokio::test]
    async fn search_missing_query() {
        let store = Arc::new(FakeMemoryStore);
        let tool = SearchMemoryTool::new(store);

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
    async fn search_returns_results() {
        let store = Arc::new(FakeMemoryStore);
        let tool = SearchMemoryTool::new(store);

        let ctx = ToolContext {
            run_id: polkagent_core::RunId::new(),
            agent_id: polkagent_core::AgentId::new(),
            step_id: polkagent_core::StepId::new(),
            grants: vec![],
            security_config: None,
        };

        let input = serde_json::json!({ "query": "test query" });
        let result = tool.execute(input, &ctx).await;
        assert!(result.is_ok());

        let r = result.unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(r.output["count"], 0);
        assert!(r.output["results"].as_array().is_some());
    }

    /// A minimal fake memory store that always returns empty results.
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
            _query: &MemoryQuery,
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
            _query: &MemoryQuery,
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
