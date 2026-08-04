//! Vector embedding types and similarity search for the memory system.
//!
//! This module provides the foundational types for semantic search beyond FTS5:
//!
//! - [`EmbeddingVector`] — dimensioned vector newtype over `Vec<f32>`
//! - [`EmbeddingModel`] — supported embedding model variants with known dimensions
//! - [`SimilarityMetric`] — distance / similarity functions (cosine, dot product, euclidean)
//! - [`VectorIndex`] — in-memory brute-force flat index for small collections
//! - [`EmbeddingProvider`] — async trait for producing embeddings from text
//! - [`MockEmbeddingProvider`] — deterministic hash-based provider for tests
//! - [`EmbeddedMemoryEntry`] — pairs a [`MemoryId`] with its embedding
//!
//! All math is pure Rust with no external numeric dependencies.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{MemoryError, MemoryResult};
use crate::types::MemoryId;

// ---------------------------------------------------------------------------
// EmbeddingVector
// ---------------------------------------------------------------------------

/// A dense floating-point vector with tracked dimensionality.
///
/// The inner `Vec<f32>` length **always** equals [`dimensions`](Self::dimensions).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EmbeddingVector {
    /// The raw vector components.
    data: Vec<f32>,
}

impl EmbeddingVector {
    /// Create a new embedding vector.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryError::InvalidOperation`] if `data` is empty.
    pub fn new(data: Vec<f32>) -> MemoryResult<Self> {
        if data.is_empty() {
            return Err(MemoryError::InvalidOperation(
                "embedding vector must not be empty".into(),
            ));
        }
        Ok(Self { data })
    }

    /// Return the number of dimensions.
    #[must_use]
    pub fn dimensions(&self) -> usize {
        self.data.len()
    }

    /// Borrow the raw float slice.
    #[must_use]
    pub fn as_slice(&self) -> &[f32] {
        &self.data
    }

    /// Consume self and return the inner `Vec<f32>`.
    #[must_use]
    pub fn into_inner(self) -> Vec<f32> {
        self.data
    }

    /// Create a zero vector of the given dimension.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryError::InvalidOperation`] if `dims` is zero.
    pub fn zeros(dims: usize) -> MemoryResult<Self> {
        if dims == 0 {
            return Err(MemoryError::InvalidOperation(
                "embedding dimensions must be > 0".into(),
            ));
        }
        Ok(Self {
            data: vec![0.0; dims],
        })
    }
}

// ---------------------------------------------------------------------------
// EmbeddingModel
// ---------------------------------------------------------------------------

/// Known embedding model variants with their native dimensionality.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EmbeddingModel {
    /// OpenAI `text-embedding-ada-002` (1536 dimensions).
    Ada002,
    /// Sentence-Transformers `all-MiniLM-L6-v2` (384 dimensions).
    AllMiniLML6,
    /// BAAI `bge-small-en-v1.5` (384 dimensions).
    BgeSmall,
    /// An arbitrary model with a custom dimension count.
    Custom(usize),
}

impl EmbeddingModel {
    /// Return the native dimensionality for this model.
    #[must_use]
    pub fn dimensions(&self) -> usize {
        match self {
            Self::Ada002 => 1536,
            Self::AllMiniLML6 | Self::BgeSmall => 384,
            Self::Custom(d) => *d,
        }
    }
}

impl std::fmt::Display for EmbeddingModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ada002 => write!(f, "ada-002"),
            Self::AllMiniLML6 => write!(f, "all-MiniLM-L6"),
            Self::BgeSmall => write!(f, "bge-small"),
            Self::Custom(d) => write!(f, "custom({d})"),
        }
    }
}

// ---------------------------------------------------------------------------
// SimilarityMetric
// ---------------------------------------------------------------------------

/// Distance or similarity metric for comparing embedding vectors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SimilarityMetric {
    /// Cosine similarity (1 = identical, 0 = orthogonal, -1 = opposite).
    Cosine,
    /// Raw dot product (unnormalised).
    DotProduct,
    /// Euclidean (L2) distance (0 = identical; lower is more similar).
    Euclidean,
}

// ---------------------------------------------------------------------------
// Vector math — pure Rust, no external deps
// ---------------------------------------------------------------------------

/// Compute the dot product of two equal-length slices.
///
/// # Errors
///
/// Returns [`MemoryError::InvalidOperation`] on dimension mismatch.
pub fn dot_product(a: &[f32], b: &[f32]) -> MemoryResult<f32> {
    if a.len() != b.len() {
        return Err(MemoryError::InvalidOperation(format!(
            "dimension mismatch: {} vs {}",
            a.len(),
            b.len()
        )));
    }
    Ok(a.iter().zip(b.iter()).map(|(x, y)| x * y).sum())
}

/// Compute the cosine similarity of two equal-length slices.
///
/// Returns `0.0` if either vector has zero magnitude.
///
/// # Errors
///
/// Returns [`MemoryError::InvalidOperation`] on dimension mismatch.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> MemoryResult<f32> {
    let dot = dot_product(a, b)?;
    let mag_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let mag_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if mag_a == 0.0 || mag_b == 0.0 {
        return Ok(0.0);
    }
    Ok(dot / (mag_a * mag_b))
}

/// Compute the Euclidean (L2) distance between two equal-length slices.
///
/// # Errors
///
/// Returns [`MemoryError::InvalidOperation`] on dimension mismatch.
pub fn euclidean_distance(a: &[f32], b: &[f32]) -> MemoryResult<f32> {
    if a.len() != b.len() {
        return Err(MemoryError::InvalidOperation(format!(
            "dimension mismatch: {} vs {}",
            a.len(),
            b.len()
        )));
    }
    let sum: f32 = a.iter().zip(b.iter()).map(|(x, y)| (x - y) * (x - y)).sum();
    Ok(sum.sqrt())
}

/// L2-normalise a vector (unit length). Returns a zero vector if magnitude is zero.
///
/// # Errors
///
/// Returns [`MemoryError::InvalidOperation`] if the input is empty.
pub fn normalize(v: &EmbeddingVector) -> MemoryResult<EmbeddingVector> {
    let mag: f32 = v.as_slice().iter().map(|x| x * x).sum::<f32>().sqrt();
    if mag == 0.0 {
        return EmbeddingVector::zeros(v.dimensions());
    }
    let normed = v.as_slice().iter().map(|x| x / mag).collect();
    EmbeddingVector::new(normed)
}

// ---------------------------------------------------------------------------
// VectorSimilarity — convenience struct grouping the math functions
// ---------------------------------------------------------------------------

/// Convenience wrapper that selects the appropriate similarity function for a
/// given [`SimilarityMetric`].
pub struct VectorSimilarity;

impl VectorSimilarity {
    /// Score two vectors using the chosen metric.
    ///
    /// For [`SimilarityMetric::Euclidean`] the raw distance is returned (lower
    /// is *more* similar). For the other metrics, higher is more similar.
    ///
    /// # Errors
    ///
    /// Propagates dimension-mismatch errors from the underlying math functions.
    pub fn score(a: &[f32], b: &[f32], metric: SimilarityMetric) -> MemoryResult<f32> {
        match metric {
            SimilarityMetric::Cosine => cosine_similarity(a, b),
            SimilarityMetric::DotProduct => dot_product(a, b),
            SimilarityMetric::Euclidean => euclidean_distance(a, b),
        }
    }
}

// ---------------------------------------------------------------------------
// EmbeddedMemoryEntry
// ---------------------------------------------------------------------------

/// Pairs a [`MemoryId`] with its precomputed embedding.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddedMemoryEntry {
    /// The memory entry this embedding belongs to.
    pub entry_id: MemoryId,
    /// The embedding vector.
    pub embedding: EmbeddingVector,
    /// When this embedded entry was created.
    pub created_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// VectorIndex — flat brute-force index
// ---------------------------------------------------------------------------

/// An in-memory flat (brute-force) vector index for small collections.
///
/// Supports linear-scan search with any [`SimilarityMetric`] and optional
/// metadata filtering.
#[derive(Debug, Clone)]
pub struct VectorIndex {
    /// Stored vectors keyed by [`MemoryId`].
    entries: Vec<IndexEntry>,
    /// Expected dimensionality for all vectors in this index.
    dimensions: usize,
}

/// Internal storage for a single indexed vector.
#[derive(Debug, Clone)]
struct IndexEntry {
    id: MemoryId,
    vector: EmbeddingVector,
    metadata: HashMap<String, String>,
}

/// A single search result from [`VectorIndex::search`].
#[derive(Debug, Clone)]
pub struct SearchResult {
    /// The identifier of the matched memory.
    pub id: MemoryId,
    /// The similarity (or distance) score.
    pub score: f32,
}

impl VectorIndex {
    /// Create a new empty index expecting vectors of `dimensions` dimensions.
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

    /// Return the number of vectors currently stored.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Return `true` if the index contains no vectors.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Insert a vector into the index.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryError::InvalidOperation`] if the vector's dimensions do
    /// not match the index's expected dimensions.
    pub fn add(
        &mut self,
        id: MemoryId,
        vector: EmbeddingVector,
        metadata: Option<HashMap<String, String>>,
    ) -> MemoryResult<()> {
        if vector.dimensions() != self.dimensions {
            return Err(MemoryError::InvalidOperation(format!(
                "vector dimension {} does not match index dimension {}",
                vector.dimensions(),
                self.dimensions
            )));
        }
        self.entries.push(IndexEntry {
            id,
            vector,
            metadata: metadata.unwrap_or_default(),
        });
        Ok(())
    }

    /// Brute-force search for the `k` most similar vectors to `query`.
    ///
    /// For [`SimilarityMetric::Cosine`] and [`SimilarityMetric::DotProduct`],
    /// results are sorted **descending** (highest similarity first).
    /// For [`SimilarityMetric::Euclidean`], results are sorted **ascending**
    /// (smallest distance first).
    ///
    /// # Errors
    ///
    /// Returns [`MemoryError::InvalidOperation`] on dimension mismatch.
    pub fn search(
        &self,
        query: &EmbeddingVector,
        k: usize,
        metric: SimilarityMetric,
    ) -> MemoryResult<Vec<SearchResult>> {
        if query.dimensions() != self.dimensions {
            return Err(MemoryError::InvalidOperation(format!(
                "query dimension {} does not match index dimension {}",
                query.dimensions(),
                self.dimensions
            )));
        }
        self.search_filtered(query, k, metric, None)
    }

    /// Like [`search`](Self::search), but with an optional metadata filter.
    ///
    /// When `filter` is `Some`, only entries whose metadata is a **superset**
    /// of the filter map are considered.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryError::InvalidOperation`] on dimension mismatch.
    pub fn search_filtered(
        &self,
        query: &EmbeddingVector,
        k: usize,
        metric: SimilarityMetric,
        filter: Option<&HashMap<String, String>>,
    ) -> MemoryResult<Vec<SearchResult>> {
        if query.dimensions() != self.dimensions {
            return Err(MemoryError::InvalidOperation(format!(
                "query dimension {} does not match index dimension {}",
                query.dimensions(),
                self.dimensions
            )));
        }

        let mut scored: Vec<SearchResult> = self
            .entries
            .iter()
            .filter(|entry| match filter {
                Some(f) => f.iter().all(|(k, v)| entry.metadata.get(k) == Some(v)),
                None => true,
            })
            .map(|entry| {
                let score =
                    VectorSimilarity::score(query.as_slice(), entry.vector.as_slice(), metric)
                        .unwrap_or(f32::NEG_INFINITY);
                SearchResult {
                    id: entry.id,
                    score,
                }
            })
            .collect();

        // Sort: descending for similarity metrics, ascending for distance.
        match metric {
            SimilarityMetric::Cosine | SimilarityMetric::DotProduct => {
                scored.sort_by(|a, b| {
                    b.score
                        .partial_cmp(&a.score)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
            }
            SimilarityMetric::Euclidean => {
                scored.sort_by(|a, b| {
                    a.score
                        .partial_cmp(&b.score)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
            }
        }

        scored.truncate(k);
        Ok(scored)
    }

    /// Remove all entries for a given [`MemoryId`] from the index.
    ///
    /// Returns the number of entries removed.
    pub fn remove(&mut self, id: MemoryId) -> usize {
        let before = self.entries.len();
        self.entries.retain(|e| e.id != id);
        before - self.entries.len()
    }
}

// ---------------------------------------------------------------------------
// EmbeddingProvider trait
// ---------------------------------------------------------------------------

/// Async trait for producing embedding vectors from text.
///
/// Implementations may call external APIs (OpenAI, local models, etc.).
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    /// Embed a single piece of text and return its vector.
    async fn embed(&self, text: &str) -> MemoryResult<EmbeddingVector>;

    /// Return the native dimensionality of vectors produced by this provider.
    fn dimensions(&self) -> usize;

    /// Return the [`EmbeddingModel`] this provider uses.
    fn model(&self) -> EmbeddingModel;
}

// ---------------------------------------------------------------------------
// MockEmbeddingProvider
// ---------------------------------------------------------------------------

/// A deterministic embedding provider that hashes input text to produce
/// repeatable vectors. Useful for testing without external API calls.
///
/// The hash-based approach guarantees:
/// - Identical text always yields the same vector.
/// - Different text almost always yields different vectors.
#[derive(Debug, Clone)]
pub struct MockEmbeddingProvider {
    dims: usize,
}

impl MockEmbeddingProvider {
    /// Create a provider that produces vectors of `dims` dimensions.
    #[must_use]
    pub fn new(dims: usize) -> Self {
        Self { dims }
    }
}

#[async_trait]
impl EmbeddingProvider for MockEmbeddingProvider {
    async fn embed(&self, text: &str) -> MemoryResult<EmbeddingVector> {
        let mut data = Vec::with_capacity(self.dims);
        for i in 0..self.dims {
            let mut hasher = DefaultHasher::new();
            text.hash(&mut hasher);
            i.hash(&mut hasher);
            let h = hasher.finish();
            // Map hash to [-1, 1] range.
            let val = ((h % 20001) as f32 / 10000.0) - 1.0;
            data.push(val);
        }
        // L2-normalise so cosine similarity tests are meaningful.
        let vec = EmbeddingVector::new(data)?;
        normalize(&vec)
    }

    fn dimensions(&self) -> usize {
        self.dims
    }

    fn model(&self) -> EmbeddingModel {
        EmbeddingModel::Custom(self.dims)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- EmbeddingVector ----------------------------------------------------

    #[test]
    fn embedding_vector_new_success() {
        let v = EmbeddingVector::new(vec![1.0, 2.0, 3.0]).unwrap();
        assert_eq!(v.dimensions(), 3);
        assert_eq!(v.as_slice(), &[1.0, 2.0, 3.0]);
    }

    #[test]
    fn embedding_vector_empty_is_err() {
        assert!(EmbeddingVector::new(vec![]).is_err());
    }

    #[test]
    fn embedding_vector_zeros() {
        let v = EmbeddingVector::zeros(4).unwrap();
        assert_eq!(v.dimensions(), 4);
        assert_eq!(v.as_slice(), &[0.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn embedding_vector_zeros_dim_zero_is_err() {
        assert!(EmbeddingVector::zeros(0).is_err());
    }

    #[test]
    fn embedding_vector_into_inner() {
        let v = EmbeddingVector::new(vec![1.0, 2.0]).unwrap();
        assert_eq!(v.into_inner(), vec![1.0, 2.0]);
    }

    #[test]
    fn embedding_vector_serde_round_trip() {
        let v = EmbeddingVector::new(vec![0.5, -0.3, 1.0]).unwrap();
        let json = serde_json::to_string(&v).unwrap();
        let back: EmbeddingVector = serde_json::from_str(&json).unwrap();
        assert_eq!(v, back);
    }

    // -- EmbeddingModel -----------------------------------------------------

    #[test]
    fn model_dimensions() {
        assert_eq!(EmbeddingModel::Ada002.dimensions(), 1536);
        assert_eq!(EmbeddingModel::AllMiniLML6.dimensions(), 384);
        assert_eq!(EmbeddingModel::BgeSmall.dimensions(), 384);
        assert_eq!(EmbeddingModel::Custom(128).dimensions(), 128);
    }

    #[test]
    fn model_display() {
        assert_eq!(EmbeddingModel::Ada002.to_string(), "ada-002");
        assert_eq!(EmbeddingModel::AllMiniLML6.to_string(), "all-MiniLM-L6");
        assert_eq!(EmbeddingModel::BgeSmall.to_string(), "bge-small");
        assert_eq!(EmbeddingModel::Custom(64).to_string(), "custom(64)");
    }

    #[test]
    fn model_serde_round_trip() {
        let m = EmbeddingModel::Ada002;
        let json = serde_json::to_string(&m).unwrap();
        let back: EmbeddingModel = serde_json::from_str(&json).unwrap();
        assert_eq!(m, back);

        let custom = EmbeddingModel::Custom(256);
        let json2 = serde_json::to_string(&custom).unwrap();
        let back2: EmbeddingModel = serde_json::from_str(&json2).unwrap();
        assert_eq!(custom, back2);
    }

    // -- Dot product --------------------------------------------------------

    #[test]
    fn dot_product_known_values() {
        // [1,2,3] . [4,5,6] = 4 + 10 + 18 = 32
        let result = dot_product(&[1.0, 2.0, 3.0], &[4.0, 5.0, 6.0]).unwrap();
        assert!((result - 32.0).abs() < 1e-6);
    }

    #[test]
    fn dot_product_orthogonal() {
        // [1,0] . [0,1] = 0
        let result = dot_product(&[1.0, 0.0], &[0.0, 1.0]).unwrap();
        assert!((result).abs() < 1e-6);
    }

    #[test]
    fn dot_product_dimension_mismatch() {
        assert!(dot_product(&[1.0, 2.0], &[1.0]).is_err());
    }

    // -- Cosine similarity --------------------------------------------------

    #[test]
    fn cosine_identical_vectors() {
        let v = [1.0, 2.0, 3.0];
        let sim = cosine_similarity(&v, &v).unwrap();
        assert!((sim - 1.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_opposite_vectors() {
        let a = [1.0, 0.0];
        let b = [-1.0, 0.0];
        let sim = cosine_similarity(&a, &b).unwrap();
        assert!((sim - (-1.0)).abs() < 1e-6);
    }

    #[test]
    fn cosine_orthogonal_vectors() {
        let a = [1.0, 0.0];
        let b = [0.0, 1.0];
        let sim = cosine_similarity(&a, &b).unwrap();
        assert!(sim.abs() < 1e-6);
    }

    #[test]
    fn cosine_zero_vector_returns_zero() {
        let a = [0.0, 0.0];
        let b = [1.0, 2.0];
        assert!((cosine_similarity(&a, &b).unwrap()).abs() < 1e-6);
    }

    #[test]
    fn cosine_dimension_mismatch() {
        assert!(cosine_similarity(&[1.0], &[1.0, 2.0]).is_err());
    }

    // -- Euclidean distance -------------------------------------------------

    #[test]
    fn euclidean_identical_vectors() {
        let v = [3.0, 4.0];
        let d = euclidean_distance(&v, &v).unwrap();
        assert!(d.abs() < 1e-6);
    }

    #[test]
    fn euclidean_known_values() {
        // distance([0,0], [3,4]) = 5
        let d = euclidean_distance(&[0.0, 0.0], &[3.0, 4.0]).unwrap();
        assert!((d - 5.0).abs() < 1e-6);
    }

    #[test]
    fn euclidean_dimension_mismatch() {
        assert!(euclidean_distance(&[1.0], &[1.0, 2.0]).is_err());
    }

    // -- Normalize ----------------------------------------------------------

    #[test]
    fn normalize_unit_vector_unchanged() {
        let v = EmbeddingVector::new(vec![1.0, 0.0, 0.0]).unwrap();
        let n = normalize(&v).unwrap();
        assert!((n.as_slice()[0] - 1.0).abs() < 1e-6);
        assert!(n.as_slice()[1].abs() < 1e-6);
        assert!(n.as_slice()[2].abs() < 1e-6);
    }

    #[test]
    fn normalize_produces_unit_length() {
        let v = EmbeddingVector::new(vec![3.0, 4.0]).unwrap();
        let n = normalize(&v).unwrap();
        let mag: f32 = n.as_slice().iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((mag - 1.0).abs() < 1e-6);
    }

    #[test]
    fn normalize_known_values() {
        // [3, 4] normalised = [0.6, 0.8]
        let v = EmbeddingVector::new(vec![3.0, 4.0]).unwrap();
        let n = normalize(&v).unwrap();
        assert!((n.as_slice()[0] - 0.6).abs() < 1e-6);
        assert!((n.as_slice()[1] - 0.8).abs() < 1e-6);
    }

    #[test]
    fn normalize_zero_vector() {
        let v = EmbeddingVector::zeros(3).unwrap();
        let n = normalize(&v).unwrap();
        assert_eq!(n.as_slice(), &[0.0, 0.0, 0.0]);
    }

    // -- VectorSimilarity ---------------------------------------------------

    #[test]
    fn vector_similarity_dispatches_correctly() {
        let a = [1.0, 0.0];
        let b = [0.0, 1.0];

        let cos = VectorSimilarity::score(&a, &b, SimilarityMetric::Cosine).unwrap();
        assert!(cos.abs() < 1e-6);

        let dot = VectorSimilarity::score(&a, &b, SimilarityMetric::DotProduct).unwrap();
        assert!(dot.abs() < 1e-6);

        let euc = VectorSimilarity::score(&a, &b, SimilarityMetric::Euclidean).unwrap();
        assert!((euc - std::f32::consts::SQRT_2).abs() < 1e-5);
    }

    // -- VectorIndex --------------------------------------------------------

    #[test]
    fn index_new_is_empty() {
        let idx = VectorIndex::new(3);
        assert!(idx.is_empty());
        assert_eq!(idx.len(), 0);
        assert_eq!(idx.dimensions(), 3);
    }

    #[test]
    fn index_add_and_len() {
        let mut idx = VectorIndex::new(2);
        let v = EmbeddingVector::new(vec![1.0, 0.0]).unwrap();
        idx.add(MemoryId::new(), v, None).unwrap();
        assert_eq!(idx.len(), 1);
        assert!(!idx.is_empty());
    }

    #[test]
    fn index_add_dimension_mismatch() {
        let mut idx = VectorIndex::new(2);
        let v = EmbeddingVector::new(vec![1.0, 2.0, 3.0]).unwrap();
        assert!(idx.add(MemoryId::new(), v, None).is_err());
    }

    #[test]
    fn index_search_empty_returns_empty() {
        let idx = VectorIndex::new(2);
        let q = EmbeddingVector::new(vec![1.0, 0.0]).unwrap();
        let results = idx.search(&q, 5, SimilarityMetric::Cosine).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn index_search_query_dimension_mismatch() {
        let idx = VectorIndex::new(3);
        let q = EmbeddingVector::new(vec![1.0, 0.0]).unwrap();
        assert!(idx.search(&q, 5, SimilarityMetric::Cosine).is_err());
    }

    #[test]
    fn index_search_cosine_top_k_ordering() {
        let mut idx = VectorIndex::new(2);

        // v1 is aligned with query
        let id1 = MemoryId::new();
        idx.add(id1, EmbeddingVector::new(vec![1.0, 0.0]).unwrap(), None)
            .unwrap();

        // v2 is at 45 degrees
        let id2 = MemoryId::new();
        idx.add(id2, EmbeddingVector::new(vec![1.0, 1.0]).unwrap(), None)
            .unwrap();

        // v3 is orthogonal
        let id3 = MemoryId::new();
        idx.add(id3, EmbeddingVector::new(vec![0.0, 1.0]).unwrap(), None)
            .unwrap();

        let q = EmbeddingVector::new(vec![1.0, 0.0]).unwrap();
        let results = idx.search(&q, 3, SimilarityMetric::Cosine).unwrap();

        assert_eq!(results.len(), 3);
        // Most similar first
        assert_eq!(results[0].id, id1);
        assert!((results[0].score - 1.0).abs() < 1e-6);
        assert_eq!(results[1].id, id2);
        assert_eq!(results[2].id, id3);
    }

    #[test]
    fn index_search_euclidean_top_k_ordering() {
        let mut idx = VectorIndex::new(2);

        let id_near = MemoryId::new();
        idx.add(id_near, EmbeddingVector::new(vec![1.0, 0.0]).unwrap(), None)
            .unwrap();

        let id_far = MemoryId::new();
        idx.add(
            id_far,
            EmbeddingVector::new(vec![10.0, 10.0]).unwrap(),
            None,
        )
        .unwrap();

        let q = EmbeddingVector::new(vec![0.0, 0.0]).unwrap();
        let results = idx.search(&q, 2, SimilarityMetric::Euclidean).unwrap();

        assert_eq!(results.len(), 2);
        // Nearest first for euclidean
        assert_eq!(results[0].id, id_near);
        assert_eq!(results[1].id, id_far);
    }

    #[test]
    fn index_search_dot_product_ordering() {
        let mut idx = VectorIndex::new(2);

        let id_big = MemoryId::new();
        idx.add(id_big, EmbeddingVector::new(vec![10.0, 0.0]).unwrap(), None)
            .unwrap();

        let id_small = MemoryId::new();
        idx.add(
            id_small,
            EmbeddingVector::new(vec![1.0, 0.0]).unwrap(),
            None,
        )
        .unwrap();

        let q = EmbeddingVector::new(vec![1.0, 0.0]).unwrap();
        let results = idx.search(&q, 2, SimilarityMetric::DotProduct).unwrap();

        assert_eq!(results[0].id, id_big);
        assert!((results[0].score - 10.0).abs() < 1e-6);
        assert_eq!(results[1].id, id_small);
    }

    #[test]
    fn index_search_k_limits_results() {
        let mut idx = VectorIndex::new(2);
        for _ in 0..10 {
            idx.add(
                MemoryId::new(),
                EmbeddingVector::new(vec![1.0, 0.0]).unwrap(),
                None,
            )
            .unwrap();
        }

        let q = EmbeddingVector::new(vec![1.0, 0.0]).unwrap();
        let results = idx.search(&q, 3, SimilarityMetric::Cosine).unwrap();
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn index_search_k_larger_than_entries() {
        let mut idx = VectorIndex::new(2);
        idx.add(
            MemoryId::new(),
            EmbeddingVector::new(vec![1.0, 0.0]).unwrap(),
            None,
        )
        .unwrap();

        let q = EmbeddingVector::new(vec![1.0, 0.0]).unwrap();
        let results = idx.search(&q, 100, SimilarityMetric::Cosine).unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn index_remove() {
        let mut idx = VectorIndex::new(2);
        let id = MemoryId::new();
        idx.add(id, EmbeddingVector::new(vec![1.0, 0.0]).unwrap(), None)
            .unwrap();
        assert_eq!(idx.len(), 1);
        assert_eq!(idx.remove(id), 1);
        assert!(idx.is_empty());
    }

    #[test]
    fn index_remove_nonexistent() {
        let mut idx = VectorIndex::new(2);
        assert_eq!(idx.remove(MemoryId::new()), 0);
    }

    #[test]
    fn index_search_with_metadata_filter() {
        let mut idx = VectorIndex::new(2);

        let id_match = MemoryId::new();
        let mut meta = HashMap::new();
        meta.insert("type".into(), "important".into());
        idx.add(
            id_match,
            EmbeddingVector::new(vec![1.0, 0.0]).unwrap(),
            Some(meta),
        )
        .unwrap();

        let id_no_match = MemoryId::new();
        let mut meta2 = HashMap::new();
        meta2.insert("type".into(), "trivial".into());
        idx.add(
            id_no_match,
            EmbeddingVector::new(vec![1.0, 0.0]).unwrap(),
            Some(meta2),
        )
        .unwrap();

        let q = EmbeddingVector::new(vec![1.0, 0.0]).unwrap();
        let mut filter = HashMap::new();
        filter.insert("type".into(), "important".into());

        let results = idx
            .search_filtered(&q, 10, SimilarityMetric::Cosine, Some(&filter))
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, id_match);
    }

    #[test]
    fn index_search_filter_no_matches() {
        let mut idx = VectorIndex::new(2);
        idx.add(
            MemoryId::new(),
            EmbeddingVector::new(vec![1.0, 0.0]).unwrap(),
            None,
        )
        .unwrap();

        let q = EmbeddingVector::new(vec![1.0, 0.0]).unwrap();
        let mut filter = HashMap::new();
        filter.insert("nonexistent".into(), "value".into());

        let results = idx
            .search_filtered(&q, 10, SimilarityMetric::Cosine, Some(&filter))
            .unwrap();
        assert!(results.is_empty());
    }

    // -- MockEmbeddingProvider ----------------------------------------------

    #[tokio::test]
    async fn mock_provider_deterministic() {
        let provider = MockEmbeddingProvider::new(8);
        let v1 = provider.embed("hello world").await.unwrap();
        let v2 = provider.embed("hello world").await.unwrap();
        assert_eq!(v1, v2);
    }

    #[tokio::test]
    async fn mock_provider_different_text_different_vectors() {
        let provider = MockEmbeddingProvider::new(8);
        let v1 = provider.embed("hello").await.unwrap();
        let v2 = provider.embed("goodbye").await.unwrap();
        assert_ne!(v1, v2);
    }

    #[tokio::test]
    async fn mock_provider_correct_dimensions() {
        let provider = MockEmbeddingProvider::new(16);
        assert_eq!(provider.dimensions(), 16);
        let v = provider.embed("test").await.unwrap();
        assert_eq!(v.dimensions(), 16);
    }

    #[tokio::test]
    async fn mock_provider_model() {
        let provider = MockEmbeddingProvider::new(64);
        assert_eq!(provider.model(), EmbeddingModel::Custom(64));
    }

    #[tokio::test]
    async fn mock_provider_produces_unit_vectors() {
        let provider = MockEmbeddingProvider::new(32);
        let v = provider.embed("testing normalisation").await.unwrap();
        let mag: f32 = v.as_slice().iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!(
            (mag - 1.0).abs() < 1e-5,
            "expected unit vector, got magnitude {mag}"
        );
    }

    // -- EmbeddedMemoryEntry ------------------------------------------------

    #[test]
    fn embedded_memory_entry_serde_round_trip() {
        let entry = EmbeddedMemoryEntry {
            entry_id: MemoryId::new(),
            embedding: EmbeddingVector::new(vec![0.1, 0.2, 0.3]).unwrap(),
            created_at: Utc::now(),
        };
        let json = serde_json::to_string(&entry).unwrap();
        let back: EmbeddedMemoryEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(entry.embedding, back.embedding);
    }

    // -- SimilarityMetric serde ---------------------------------------------

    #[test]
    fn similarity_metric_serde_round_trip() {
        for metric in [
            SimilarityMetric::Cosine,
            SimilarityMetric::DotProduct,
            SimilarityMetric::Euclidean,
        ] {
            let json = serde_json::to_string(&metric).unwrap();
            let back: SimilarityMetric = serde_json::from_str(&json).unwrap();
            assert_eq!(metric, back);
        }
    }
}
