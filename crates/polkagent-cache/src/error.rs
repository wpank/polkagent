use std::fmt;

/// Errors that can occur during cache operations.
#[derive(Debug, thiserror::Error)]
pub enum CacheError {
    /// The requested key was not found in the cache.
    #[error("cache key not found: {key}")]
    NotFound { key: String },

    /// A serialization or deserialization error occurred.
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// The cache capacity has been exceeded and eviction failed.
    #[error("cache capacity exceeded: max {max_entries} entries")]
    CapacityExceeded { max_entries: usize },

    /// An eviction callback returned an error.
    #[error("eviction callback failed for key {key}: {reason}")]
    EvictionCallbackFailed { key: String, reason: String },

    /// The cache store is in an invalid state.
    #[error("invalid cache state: {0}")]
    InvalidState(String),

    /// A lock could not be acquired.
    #[error("lock acquisition failed: {0}")]
    LockFailed(String),

    /// A loader function failed during cache-aside fetch.
    #[error("loader failed for key {key}: {reason}")]
    LoaderFailed { key: String, reason: String },
}

/// A specialized Result type for cache operations.
pub type CacheResult<T> = Result<T, CacheError>;

impl CacheError {
    /// Returns `true` if this is a `NotFound` error.
    pub fn is_not_found(&self) -> bool {
        matches!(self, Self::NotFound { .. })
    }

    /// Returns `true` if this is a `LoaderFailed` error.
    pub fn is_loader_failed(&self) -> bool {
        matches!(self, Self::LoaderFailed { .. })
    }
}

/// Allow comparing `CacheError` variants in tests.
impl PartialEq for CacheError {
    fn eq(&self, other: &Self) -> bool {
        // Compare via Debug representation for simplicity in tests.
        fmt::format(format_args!("{self:?}")) == fmt::format(format_args!("{other:?}"))
    }
}
