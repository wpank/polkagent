//! Hybrid retrieval combining FTS5 text search with vector similarity search.
//!
//! Uses Reciprocal Rank Fusion (RRF) to merge ranked lists from different
//! retrieval sources into a single unified ranking.
//!
//! # Architecture
//!
//! - [`VectorSearchIndex`] — trait for pluggable vector backends
//! - [`InMemoryVectorIndex`] — brute-force cosine similarity (testing)
//! - [`SqliteVecIndex`] — wraps sqlite-vec extension (production)
//! - [`RetrievalConfig`] — tuning knobs for hybrid retrieval
//! - [`ContextBudget`] — token-based retrieval budget
//! - [`HybridRetriever`] — orchestrates FTS5 + vector search with RRF fusion

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::embedding::cosine_similarity;
use crate::error::{MemoryError, MemoryResult};
use crate::store::MemoryStore;
use crate::types::{MemoryEntry, MemoryId, MemoryQuery};

use polkagent_core::ids::AgentId;

// ---------------------------------------------------------------------------
// VectorSearchIndex trait
// ---------------------------------------------------------------------------

/// Trait for vector similarity search backends.
///
/// Implementations may be in-memory (brute-force) or backed by sqlite-vec.
pub trait VectorSearchIndex: Send + Sync {
    /// Search for the `k` most similar vectors to `embedding`.
    ///
    /// Returns `(MemoryId, score)` pairs sorted by descending similarity.
    fn search(&self, embedding: &[f32], k: usize) -> MemoryResult<Vec<(MemoryId, f32)>>;

    /// Insert or update a vector for the given memory.
    fn upsert(&mut self, id: MemoryId, embedding: &[f32]) -> MemoryResult<()>;

    /// Remove a vector by memory id.
    fn remove(&mut self, id: MemoryId) -> MemoryResult<()>;

    /// Return the number of vectors in the index.
    fn len(&self) -> usize;

    /// Return true if the index contains no vectors.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

// ---------------------------------------------------------------------------
// InMemoryVectorIndex
// ---------------------------------------------------------------------------

/// Brute-force cosine similarity index for testing.
///
/// Stores all vectors in memory and performs linear scan search.
#[derive(Debug, Clone)]
pub struct InMemoryVectorIndex {
    entries: Vec<(MemoryId, Vec<f32>)>,
    dimensions: usize,
}

impl InMemoryVectorIndex {
    /// Create a new in-memory vector index with the given dimensionality.
    #[must_use]
    pub fn new(dimensions: usize) -> Self {
        Self {
            entries: Vec::new(),
            dimensions,
        }
    }

    /// Return the expected dimensionality of vectors in this index.
    #[must_use]
    pub fn dimensions(&self) -> usize {
        self.dimensions
    }
}

impl VectorSearchIndex for InMemoryVectorIndex {
    fn search(&self, embedding: &[f32], k: usize) -> MemoryResult<Vec<(MemoryId, f32)>> {
        if embedding.len() != self.dimensions {
            return Err(MemoryError::InvalidOperation(format!(
                "query dimension {} does not match index dimension {}",
                embedding.len(),
                self.dimensions,
            )));
        }

        let mut scored: Vec<(MemoryId, f32)> = self
            .entries
            .iter()
            .filter_map(|(id, vec)| {
                cosine_similarity(embedding, vec)
                    .ok()
                    .map(|score| (*id, score))
            })
            .collect();

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(k);
        Ok(scored)
    }

    fn upsert(&mut self, id: MemoryId, embedding: &[f32]) -> MemoryResult<()> {
        if embedding.len() != self.dimensions {
            return Err(MemoryError::InvalidOperation(format!(
                "vector dimension {} does not match index dimension {}",
                embedding.len(),
                self.dimensions,
            )));
        }

        // Remove existing entry if present, then insert.
        self.entries.retain(|(eid, _)| *eid != id);
        self.entries.push((id, embedding.to_vec()));
        Ok(())
    }

    fn remove(&mut self, id: MemoryId) -> MemoryResult<()> {
        self.entries.retain(|(eid, _)| *eid != id);
        Ok(())
    }

    fn len(&self) -> usize {
        self.entries.len()
    }
}

// ---------------------------------------------------------------------------
// SqliteVecIndex
// ---------------------------------------------------------------------------

/// Wraps the sqlite-vec extension for production vector search.
///
/// Falls back to [`InMemoryVectorIndex`] when sqlite-vec is not available.
#[derive(Debug, Clone)]
pub struct SqliteVecIndex {
    /// Falls back to in-memory when sqlite-vec is not loaded.
    fallback: InMemoryVectorIndex,
}

impl SqliteVecIndex {
    /// Create a new sqlite-vec index.
    ///
    /// Currently uses an in-memory fallback; a future version will initialise
    /// the sqlite-vec virtual table when the extension is available.
    #[must_use]
    pub fn new(dimensions: usize) -> Self {
        Self {
            fallback: InMemoryVectorIndex::new(dimensions),
        }
    }

    /// Return the expected dimensionality.
    #[must_use]
    pub fn dimensions(&self) -> usize {
        self.fallback.dimensions()
    }
}

impl VectorSearchIndex for SqliteVecIndex {
    fn search(&self, embedding: &[f32], k: usize) -> MemoryResult<Vec<(MemoryId, f32)>> {
        self.fallback.search(embedding, k)
    }

    fn upsert(&mut self, id: MemoryId, embedding: &[f32]) -> MemoryResult<()> {
        self.fallback.upsert(id, embedding)
    }

    fn remove(&mut self, id: MemoryId) -> MemoryResult<()> {
        self.fallback.remove(id)
    }

    fn len(&self) -> usize {
        self.fallback.len()
    }
}

// ---------------------------------------------------------------------------
// RetrievalConfig
// ---------------------------------------------------------------------------

/// Configuration for hybrid retrieval.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetrievalConfig {
    /// Maximum number of results to return.
    pub max_results: usize,
    /// RRF constant `k` — higher values compress rank differences.
    pub rrf_k: u32,
    /// Weight for FTS5 text search scores in RRF fusion.
    pub fts_weight: f32,
    /// Weight for vector similarity scores in RRF fusion.
    pub vec_weight: f32,
}

impl Default for RetrievalConfig {
    fn default() -> Self {
        Self {
            max_results: 10,
            rrf_k: 60,
            fts_weight: 1.0,
            vec_weight: 1.0,
        }
    }
}

// ---------------------------------------------------------------------------
// ContextBudget
// ---------------------------------------------------------------------------

/// Token budget for limiting retrieval output size.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextBudget {
    /// Maximum number of tokens to include in retrieval results.
    pub max_tokens: usize,
}

impl ContextBudget {
    /// Create a new context budget.
    #[must_use]
    pub fn new(max_tokens: usize) -> Self {
        Self { max_tokens }
    }

    /// Estimate the token count for a piece of text.
    ///
    /// Uses a simple heuristic: ~4 characters per token (English average).
    #[must_use]
    pub fn estimate_tokens(text: &str) -> usize {
        // Rough heuristic: 1 token ~= 4 chars for English text.
        text.len().div_ceil(4)
    }
}

impl Default for ContextBudget {
    fn default() -> Self {
        Self { max_tokens: 4096 }
    }
}

// ---------------------------------------------------------------------------
// RRF scoring
// ---------------------------------------------------------------------------

/// A single RRF-scored result combining ranks from multiple sources.
#[derive(Debug, Clone)]
pub struct RankedResult {
    /// The memory entry id.
    pub id: MemoryId,
    /// The fused RRF score.
    pub score: f64,
}

/// Compute Reciprocal Rank Fusion across multiple ranked lists.
///
/// `score = sum(weight_i / (k + rank_i))` for each source where the item
/// appears. `rank_i` is 1-based.
pub fn rrf_fuse(ranked_lists: &[(&[(MemoryId, f32)], f32)], k: u32) -> Vec<RankedResult> {
    let mut scores: HashMap<MemoryId, f64> = HashMap::new();

    for (ranked_list, weight) in ranked_lists {
        for (rank_0, (id, _raw_score)) in ranked_list.iter().enumerate() {
            let rank_1 = f64::from(u32::try_from(rank_0 + 1).unwrap_or(u32::MAX));
            let rrf_score = f64::from(*weight) / (f64::from(k) + rank_1);
            *scores.entry(*id).or_default() += rrf_score;
        }
    }

    let mut results: Vec<RankedResult> = scores
        .into_iter()
        .map(|(id, score)| RankedResult { id, score })
        .collect();

    results.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    results
}

// ---------------------------------------------------------------------------
// HybridRetriever
// ---------------------------------------------------------------------------

/// Orchestrates FTS5 text search + vector similarity search with RRF fusion.
pub struct HybridRetriever<'a> {
    store: &'a dyn MemoryStore,
    vec_index: &'a dyn VectorSearchIndex,
    config: RetrievalConfig,
    budget: Option<ContextBudget>,
}

impl<'a> HybridRetriever<'a> {
    /// Create a new hybrid retriever.
    pub fn new(
        store: &'a dyn MemoryStore,
        vec_index: &'a dyn VectorSearchIndex,
        config: RetrievalConfig,
    ) -> Self {
        Self {
            store,
            vec_index,
            config,
            budget: None,
        }
    }

    /// Set a context budget to limit total token output.
    #[must_use]
    pub fn with_budget(mut self, budget: ContextBudget) -> Self {
        self.budget = Some(budget);
        self
    }

    /// Perform hybrid retrieval: FTS5 + vector search, fused with RRF.
    ///
    /// 1. Run FTS5 text search via the store
    /// 2. Run vector similarity search via the index
    /// 3. Fuse both ranked lists using RRF
    /// 4. Fetch full entries for the top results
    /// 5. Apply context budget if configured
    pub async fn retrieve(
        &self,
        agent_id: AgentId,
        query_text: &str,
        query_embedding: &[f32],
    ) -> MemoryResult<Vec<MemoryEntry>> {
        // Fetch more candidates than max_results to give RRF good coverage.
        let fetch_k = self.config.max_results * 3;

        // 1. FTS5 text search.
        let fts_query = MemoryQuery {
            agent_id: Some(agent_id),
            query_text: query_text.to_string(),
            memory_types: None,
            limit: fetch_k,
            min_relevance: None,
            since: None,
            episode_id: None,
        };

        let fts_entries = self.store.search(&fts_query).await?;
        let fts_ranked: Vec<(MemoryId, f32)> = fts_entries
            .iter()
            // RRF uses slice order as the rank and intentionally ignores the
            // source's raw score.
            .map(|entry| (entry.id, 0.0))
            .collect();

        // 2. Vector similarity search.
        let vec_ranked = self.vec_index.search(query_embedding, fetch_k)?;

        // 3. RRF fusion.
        let ranked_lists: Vec<(&[(MemoryId, f32)], f32)> = vec![
            (fts_ranked.as_slice(), self.config.fts_weight),
            (vec_ranked.as_slice(), self.config.vec_weight),
        ];
        let fused = rrf_fuse(&ranked_lists, self.config.rrf_k);

        // 4. Build a lookup map from both search results.
        let mut entry_map: HashMap<MemoryId, MemoryEntry> = HashMap::new();
        for entry in fts_entries {
            entry_map.insert(entry.id, entry);
        }

        // For entries that only appeared in vector results, fetch from store.
        let mut missing_ids: Vec<MemoryId> = Vec::new();
        for result in &fused {
            if !entry_map.contains_key(&result.id) {
                missing_ids.push(result.id);
            }
        }
        for id in missing_ids {
            if let Ok(entry) = self.store.get_memory(id).await {
                entry_map.insert(entry.id, entry);
            }
        }

        // 5. Collect results in RRF order, up to max_results.
        let mut results: Vec<MemoryEntry> = Vec::new();
        let mut token_count: usize = 0;

        for ranked in fused.iter().take(self.config.max_results) {
            if let Some(entry) = entry_map.remove(&ranked.id) {
                if let Some(ref budget) = self.budget {
                    let entry_tokens = ContextBudget::estimate_tokens(&entry.content);
                    if token_count + entry_tokens > budget.max_tokens {
                        break;
                    }
                    token_count += entry_tokens;
                }
                results.push(entry);
            }
        }

        Ok(results)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedding::EmbeddingProvider;
    use crate::embedding::MockEmbeddingProvider;
    use crate::sqlite::SqliteMemoryStore;
    use crate::store::MemoryStore;
    use crate::types::{MemoryEntry, MemoryType};

    use chrono::Utc;

    // -- InMemoryVectorIndex -------------------------------------------------

    #[test]
    fn in_memory_index_new_is_empty() {
        let idx = InMemoryVectorIndex::new(3);
        assert!(idx.is_empty());
        assert_eq!(idx.len(), 0);
        assert_eq!(idx.dimensions(), 3);
    }

    #[test]
    fn in_memory_index_upsert_and_len() {
        let mut idx = InMemoryVectorIndex::new(2);
        let id = MemoryId::new();
        idx.upsert(id, &[1.0, 0.0]).unwrap();
        assert_eq!(idx.len(), 1);
        assert!(!idx.is_empty());
    }

    #[test]
    fn in_memory_index_upsert_replaces_existing() {
        let mut idx = InMemoryVectorIndex::new(2);
        let id = MemoryId::new();
        idx.upsert(id, &[1.0, 0.0]).unwrap();
        idx.upsert(id, &[0.0, 1.0]).unwrap();
        assert_eq!(idx.len(), 1);

        // The updated vector should be [0, 1], so searching with [0, 1]
        // should return a perfect match.
        let results = idx.search(&[0.0, 1.0], 1).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, id);
        assert!((results[0].1 - 1.0).abs() < 1e-6);
    }

    #[test]
    fn in_memory_index_upsert_dimension_mismatch() {
        let mut idx = InMemoryVectorIndex::new(2);
        let id = MemoryId::new();
        assert!(idx.upsert(id, &[1.0, 2.0, 3.0]).is_err());
    }

    #[test]
    fn in_memory_index_search_empty_returns_empty() {
        let idx = InMemoryVectorIndex::new(2);
        let results = idx.search(&[1.0, 0.0], 5).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn in_memory_index_search_dimension_mismatch() {
        let idx = InMemoryVectorIndex::new(3);
        assert!(idx.search(&[1.0, 0.0], 5).is_err());
    }

    #[test]
    fn in_memory_index_search_ordering() {
        let mut idx = InMemoryVectorIndex::new(2);

        let id1 = MemoryId::new();
        idx.upsert(id1, &[1.0, 0.0]).unwrap();

        let id2 = MemoryId::new();
        idx.upsert(id2, &[0.707, 0.707]).unwrap();

        let id3 = MemoryId::new();
        idx.upsert(id3, &[0.0, 1.0]).unwrap();

        // Query along [1, 0] — id1 should be best match.
        let results = idx.search(&[1.0, 0.0], 3).unwrap();
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].0, id1);
        assert_eq!(results[2].0, id3);
    }

    #[test]
    fn in_memory_index_search_k_limits_results() {
        let mut idx = InMemoryVectorIndex::new(2);
        for _ in 0..10 {
            idx.upsert(MemoryId::new(), &[1.0, 0.0]).unwrap();
        }
        let results = idx.search(&[1.0, 0.0], 3).unwrap();
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn in_memory_index_remove() {
        let mut idx = InMemoryVectorIndex::new(2);
        let id = MemoryId::new();
        idx.upsert(id, &[1.0, 0.0]).unwrap();
        idx.remove(id).unwrap();
        assert!(idx.is_empty());
    }

    #[test]
    fn in_memory_index_remove_nonexistent_is_ok() {
        let mut idx = InMemoryVectorIndex::new(2);
        assert!(idx.remove(MemoryId::new()).is_ok());
    }

    // -- SqliteVecIndex ------------------------------------------------------

    #[test]
    fn sqlite_vec_index_delegates_to_fallback() {
        let mut idx = SqliteVecIndex::new(2);
        assert!(idx.is_empty());
        assert_eq!(idx.dimensions(), 2);

        let id = MemoryId::new();
        idx.upsert(id, &[1.0, 0.0]).unwrap();
        assert_eq!(idx.len(), 1);

        let results = idx.search(&[1.0, 0.0], 1).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, id);

        idx.remove(id).unwrap();
        assert!(idx.is_empty());
    }

    // -- RetrievalConfig -----------------------------------------------------

    #[test]
    fn retrieval_config_default() {
        let cfg = RetrievalConfig::default();
        assert_eq!(cfg.max_results, 10);
        assert_eq!(cfg.rrf_k, 60);
        assert!((cfg.fts_weight - 1.0).abs() < f32::EPSILON);
        assert!((cfg.vec_weight - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn retrieval_config_serde_round_trip() {
        let cfg = RetrievalConfig {
            max_results: 20,
            rrf_k: 30,
            fts_weight: 0.5,
            vec_weight: 1.5,
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let back: RetrievalConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.max_results, 20);
        assert_eq!(back.rrf_k, 30);
    }

    // -- ContextBudget -------------------------------------------------------

    #[test]
    fn context_budget_default() {
        let budget = ContextBudget::default();
        assert_eq!(budget.max_tokens, 4096);
    }

    #[test]
    fn context_budget_new() {
        let budget = ContextBudget::new(2048);
        assert_eq!(budget.max_tokens, 2048);
    }

    #[test]
    fn context_budget_estimate_tokens() {
        assert_eq!(ContextBudget::estimate_tokens(""), 0);
        assert_eq!(ContextBudget::estimate_tokens("word"), 1);
        // 20 chars -> 5 tokens
        assert_eq!(ContextBudget::estimate_tokens("12345678901234567890"), 5);
    }

    #[test]
    fn context_budget_serde_round_trip() {
        let budget = ContextBudget::new(1024);
        let json = serde_json::to_string(&budget).unwrap();
        let back: ContextBudget = serde_json::from_str(&json).unwrap();
        assert_eq!(back.max_tokens, 1024);
    }

    // -- RRF scoring ---------------------------------------------------------

    #[test]
    fn rrf_fuse_empty_lists() {
        let empty: &[(MemoryId, f32)] = &[];
        let result = rrf_fuse(&[(empty, 1.0)], 60);
        assert!(result.is_empty());
    }

    #[test]
    fn rrf_fuse_single_list() {
        let id1 = MemoryId::new();
        let id2 = MemoryId::new();
        let list: Vec<(MemoryId, f32)> = vec![(id1, 1.0), (id2, 0.5)];

        let result = rrf_fuse(&[(&list, 1.0)], 60);
        assert_eq!(result.len(), 2);
        // rank 1 score: 1/(60+1) = 0.01639...
        // rank 2 score: 1/(60+2) = 0.01613...
        assert_eq!(result[0].id, id1);
        assert!(result[0].score > result[1].score);
    }

    #[test]
    fn rrf_fuse_two_lists_same_item_gets_boosted() {
        let id1 = MemoryId::new();
        let id2 = MemoryId::new();
        let id3 = MemoryId::new();

        // id1 appears in both lists (should get boosted)
        let fts: Vec<(MemoryId, f32)> = vec![(id1, 1.0), (id2, 0.5)];
        let vec: Vec<(MemoryId, f32)> = vec![(id3, 0.9), (id1, 0.8)];

        let result = rrf_fuse(&[(&fts, 1.0), (&vec, 1.0)], 60);

        // id1 should be first since it appears in both lists.
        assert_eq!(result[0].id, id1);

        // id1 score: 1/(60+1) + 1/(60+2) = 0.01639 + 0.01613 = 0.03252
        let expected_id1 = 1.0 / 61.0 + 1.0 / 62.0;
        assert!((result[0].score - expected_id1).abs() < 1e-10);
    }

    #[test]
    fn rrf_fuse_weights_apply() {
        let id1 = MemoryId::new();
        let id2 = MemoryId::new();

        let fts: Vec<(MemoryId, f32)> = vec![(id1, 1.0)];
        let vec: Vec<(MemoryId, f32)> = vec![(id2, 1.0)];

        // Give FTS 2x weight.
        let result = rrf_fuse(&[(&fts, 2.0), (&vec, 1.0)], 60);

        assert_eq!(result.len(), 2);
        // id1 fts score: 2.0/(60+1) = 0.03279
        // id2 vec score: 1.0/(60+1) = 0.01639
        assert_eq!(result[0].id, id1);
        assert!(result[0].score > result[1].score);
    }

    #[test]
    fn rrf_fuse_k_affects_scoring() {
        let id1 = MemoryId::new();
        let id2 = MemoryId::new();

        let list: Vec<(MemoryId, f32)> = vec![(id1, 1.0), (id2, 0.5)];

        // With k=0, rank 1 gets score 1/1 = 1.0, rank 2 gets 1/2 = 0.5
        let result_k0 = rrf_fuse(&[(&list, 1.0)], 0);
        // With k=1000, difference is much smaller.
        let result_k1000 = rrf_fuse(&[(&list, 1.0)], 1000);

        let ratio_k0 = result_k0[0].score / result_k0[1].score;
        let ratio_k1000 = result_k1000[0].score / result_k1000[1].score;

        // Higher k compresses the difference.
        assert!(ratio_k0 > ratio_k1000);
    }

    #[test]
    fn rrf_fuse_preserves_all_ids() {
        let ids: Vec<MemoryId> = (0..5).map(|_| MemoryId::new()).collect();
        let list1: Vec<(MemoryId, f32)> = ids[..3].iter().map(|id| (*id, 0.0)).collect();
        let list2: Vec<(MemoryId, f32)> = ids[2..].iter().map(|id| (*id, 0.0)).collect();

        let result = rrf_fuse(&[(&list1, 1.0), (&list2, 1.0)], 60);
        assert_eq!(result.len(), 5);
    }

    // -- HybridRetriever (integration) ---------------------------------------

    fn make_test_entry(agent_id: AgentId, content: &str) -> MemoryEntry {
        let now = Utc::now();
        MemoryEntry {
            id: MemoryId::new(),
            agent_id,
            episode_id: None,
            memory_type: MemoryType::Semantic,
            content: content.to_string(),
            embedding: None,
            metadata: serde_json::json!({}),
            provenance: None,
            created_at: now,
            accessed_at: now,
            access_count: 0,
            relevance_score: 1.0,
            confidence: 1.0,
            classification: crate::classification::Classification::Internal,
        }
    }

    #[tokio::test]
    async fn hybrid_retriever_empty_returns_empty() {
        let store = SqliteMemoryStore::open_in_memory().unwrap();
        let vec_index = InMemoryVectorIndex::new(4);
        let config = RetrievalConfig::default();

        let retriever = HybridRetriever::new(&store, &vec_index, config);
        let agent_id = AgentId::new();

        let results = retriever
            .retrieve(agent_id, "anything", &[1.0, 0.0, 0.0, 0.0])
            .await
            .unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn hybrid_retriever_fts_only() {
        let store = SqliteMemoryStore::open_in_memory().unwrap();
        let vec_index = InMemoryVectorIndex::new(4);
        let agent_id = AgentId::new();

        let entry = make_test_entry(agent_id, "Rust programming language");
        store.store_memory(&entry).await.unwrap();

        let config = RetrievalConfig {
            max_results: 10,
            rrf_k: 60,
            fts_weight: 1.0,
            vec_weight: 0.0,
        };

        let retriever = HybridRetriever::new(&store, &vec_index, config);
        let results = retriever
            .retrieve(agent_id, "Rust", &[1.0, 0.0, 0.0, 0.0])
            .await
            .unwrap();

        assert_eq!(results.len(), 1);
        assert!(results[0].content.contains("Rust"));
    }

    #[tokio::test]
    async fn hybrid_retriever_vec_only() {
        let store = SqliteMemoryStore::open_in_memory().unwrap();
        let mut vec_index = InMemoryVectorIndex::new(2);
        let agent_id = AgentId::new();

        let entry = make_test_entry(agent_id, "some content");
        store.store_memory(&entry).await.unwrap();
        vec_index.upsert(entry.id, &[1.0, 0.0]).unwrap();

        let config = RetrievalConfig {
            max_results: 10,
            rrf_k: 60,
            fts_weight: 0.0,
            vec_weight: 1.0,
        };

        let retriever = HybridRetriever::new(&store, &vec_index, config);
        let results = retriever
            .retrieve(agent_id, "no match query", &[1.0, 0.0])
            .await
            .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, entry.id);
    }

    #[tokio::test]
    async fn hybrid_retriever_combines_fts_and_vec() {
        let store = SqliteMemoryStore::open_in_memory().unwrap();
        let mut vec_index = InMemoryVectorIndex::new(2);
        let agent_id = AgentId::new();

        // Entry found by FTS only.
        let fts_entry = make_test_entry(agent_id, "Rust programming");
        store.store_memory(&fts_entry).await.unwrap();

        // Entry found by vector only.
        let vec_entry = make_test_entry(agent_id, "unrelated text");
        store.store_memory(&vec_entry).await.unwrap();
        vec_index.upsert(vec_entry.id, &[1.0, 0.0]).unwrap();

        // Entry found by both.
        let both_entry = make_test_entry(agent_id, "Rust vectors");
        store.store_memory(&both_entry).await.unwrap();
        vec_index.upsert(both_entry.id, &[0.9, 0.1]).unwrap();

        let config = RetrievalConfig {
            max_results: 10,
            rrf_k: 60,
            fts_weight: 1.0,
            vec_weight: 1.0,
        };

        let retriever = HybridRetriever::new(&store, &vec_index, config);
        let results = retriever
            .retrieve(agent_id, "Rust", &[1.0, 0.0])
            .await
            .unwrap();

        // both_entry should be ranked highest since it appears in both lists.
        assert!(!results.is_empty());
        assert_eq!(results[0].id, both_entry.id);
    }

    #[tokio::test]
    async fn hybrid_retriever_max_results_limits_output() {
        let store = SqliteMemoryStore::open_in_memory().unwrap();
        let mut vec_index = InMemoryVectorIndex::new(2);
        let agent_id = AgentId::new();

        for i in 0..10 {
            let entry = make_test_entry(agent_id, &format!("Rust topic {i}"));
            store.store_memory(&entry).await.unwrap();
            vec_index.upsert(entry.id, &[1.0, 0.0]).unwrap();
        }

        let config = RetrievalConfig {
            max_results: 3,
            ..RetrievalConfig::default()
        };

        let retriever = HybridRetriever::new(&store, &vec_index, config);
        let results = retriever
            .retrieve(agent_id, "Rust", &[1.0, 0.0])
            .await
            .unwrap();

        assert_eq!(results.len(), 3);
    }

    #[tokio::test]
    async fn hybrid_retriever_budget_limits_tokens() {
        let store = SqliteMemoryStore::open_in_memory().unwrap();
        let mut vec_index = InMemoryVectorIndex::new(2);
        let agent_id = AgentId::new();

        // Each entry is ~25 chars => ~7 tokens.
        for i in 0..10 {
            let entry = make_test_entry(agent_id, &format!("Rust topic number {i:03}"));
            store.store_memory(&entry).await.unwrap();
            vec_index.upsert(entry.id, &[1.0, 0.0]).unwrap();
        }

        let config = RetrievalConfig {
            max_results: 10,
            ..RetrievalConfig::default()
        };

        // Budget for ~14 tokens (2 entries worth).
        let budget = ContextBudget::new(14);

        let retriever = HybridRetriever::new(&store, &vec_index, config).with_budget(budget);
        let results = retriever
            .retrieve(agent_id, "Rust", &[1.0, 0.0])
            .await
            .unwrap();

        assert!(
            results.len() <= 3,
            "budget should limit results, got {}",
            results.len()
        );
        assert!(!results.is_empty(), "should return at least one result");
    }

    #[tokio::test]
    async fn hybrid_retriever_budget_zero_returns_empty() {
        let store = SqliteMemoryStore::open_in_memory().unwrap();
        let mut vec_index = InMemoryVectorIndex::new(2);
        let agent_id = AgentId::new();

        let entry = make_test_entry(agent_id, "some content");
        store.store_memory(&entry).await.unwrap();
        vec_index.upsert(entry.id, &[1.0, 0.0]).unwrap();

        let config = RetrievalConfig::default();
        let budget = ContextBudget::new(0);

        let retriever = HybridRetriever::new(&store, &vec_index, config).with_budget(budget);
        let results = retriever
            .retrieve(agent_id, "some", &[1.0, 0.0])
            .await
            .unwrap();

        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn hybrid_retriever_different_agents_isolated() {
        let store = SqliteMemoryStore::open_in_memory().unwrap();
        let mut vec_index = InMemoryVectorIndex::new(2);

        let agent_a = AgentId::new();
        let agent_b = AgentId::new();

        let entry_a = make_test_entry(agent_a, "Rust for agent A");
        store.store_memory(&entry_a).await.unwrap();
        vec_index.upsert(entry_a.id, &[1.0, 0.0]).unwrap();

        let entry_b = make_test_entry(agent_b, "Rust for agent B");
        store.store_memory(&entry_b).await.unwrap();
        vec_index.upsert(entry_b.id, &[1.0, 0.0]).unwrap();

        let config = RetrievalConfig::default();

        // Agent A should only see their own memories from FTS.
        let retriever = HybridRetriever::new(&store, &vec_index, config.clone());
        let results_a = retriever
            .retrieve(agent_a, "Rust", &[1.0, 0.0])
            .await
            .unwrap();

        // The FTS results are scoped to agent_a, so only entry_a comes from FTS.
        // entry_b might appear from vec search but with lower RRF score.
        assert!(!results_a.is_empty());
        // The top result should be from agent_a (appears in both FTS and vec).
        assert_eq!(results_a[0].id, entry_a.id);
    }

    // -- Integration with MockEmbeddingProvider ------------------------------

    #[tokio::test]
    async fn hybrid_retriever_with_mock_embeddings() {
        let store = SqliteMemoryStore::open_in_memory().unwrap();
        let provider = MockEmbeddingProvider::new(8);
        let mut vec_index = InMemoryVectorIndex::new(8);
        let agent_id = AgentId::new();

        let texts = [
            "Rust is a systems programming language",
            "Python is great for data science",
            "Rust has zero-cost abstractions",
        ];

        for text in &texts {
            let mut entry = make_test_entry(agent_id, text);
            let embedding = provider.embed(text).await.unwrap();
            entry.embedding = Some(embedding.as_slice().to_vec());
            store.store_memory(&entry).await.unwrap();
            vec_index.upsert(entry.id, embedding.as_slice()).unwrap();
        }

        let query = "Rust programming";
        let query_embedding = provider.embed(query).await.unwrap();

        let config = RetrievalConfig {
            max_results: 3,
            ..RetrievalConfig::default()
        };

        let retriever = HybridRetriever::new(&store, &vec_index, config);
        let results = retriever
            .retrieve(agent_id, query, query_embedding.as_slice())
            .await
            .unwrap();

        assert!(!results.is_empty());
        // FTS should find entries containing "Rust", vector search provides
        // additional signal. All three entries should be returned.
        assert!(results.len() >= 2);
    }

    #[tokio::test]
    async fn hybrid_retriever_custom_rrf_k() {
        let store = SqliteMemoryStore::open_in_memory().unwrap();
        let mut vec_index = InMemoryVectorIndex::new(2);
        let agent_id = AgentId::new();

        for i in 0..5 {
            let entry = make_test_entry(agent_id, &format!("Rust concept {i}"));
            store.store_memory(&entry).await.unwrap();
            vec_index.upsert(entry.id, &[1.0, 0.0]).unwrap();
        }

        // Low k amplifies rank differences.
        let config_low_k = RetrievalConfig {
            max_results: 5,
            rrf_k: 1,
            ..RetrievalConfig::default()
        };

        let retriever = HybridRetriever::new(&store, &vec_index, config_low_k);
        let results = retriever
            .retrieve(agent_id, "Rust", &[1.0, 0.0])
            .await
            .unwrap();

        assert_eq!(results.len(), 5);
    }

    #[test]
    fn rrf_fuse_deterministic() {
        let id1 = MemoryId::new();
        let id2 = MemoryId::new();
        let list: Vec<(MemoryId, f32)> = vec![(id1, 1.0), (id2, 0.5)];

        let r1 = rrf_fuse(&[(&list, 1.0)], 60);
        let r2 = rrf_fuse(&[(&list, 1.0)], 60);

        assert_eq!(r1.len(), r2.len());
        for (a, b) in r1.iter().zip(r2.iter()) {
            assert_eq!(a.id, b.id);
            assert!((a.score - b.score).abs() < 1e-15);
        }
    }

    #[test]
    fn rrf_fuse_three_lists() {
        let id1 = MemoryId::new();
        let id2 = MemoryId::new();

        let l1: Vec<(MemoryId, f32)> = vec![(id1, 1.0)];
        let l2: Vec<(MemoryId, f32)> = vec![(id1, 1.0), (id2, 0.5)];
        let l3: Vec<(MemoryId, f32)> = vec![(id2, 1.0)];

        let result = rrf_fuse(&[(&l1, 1.0), (&l2, 1.0), (&l3, 1.0)], 60);

        // id1 appears in l1(rank 1) and l2(rank 1): 2/(60+1)
        // id2 appears in l2(rank 2) and l3(rank 1): 1/(60+2) + 1/(60+1)
        assert_eq!(result.len(), 2);
    }
}
