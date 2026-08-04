//! Async trait for the service registry store.

use async_trait::async_trait;

use crate::types::{CreateListingRequest, ListingId, SearchFilter, ServiceListing};

/// Async storage trait for the marketplace service registry.
///
/// Implementations must be `Send + Sync` so that `Arc<dyn ServiceRegistryStore>`
/// can be shared across Axum handlers.
#[async_trait]
pub trait ServiceRegistryStore: Send + Sync {
    /// Insert a new service listing, returning the created listing.
    async fn create_listing(
        &self,
        request: CreateListingRequest,
    ) -> Result<ServiceListing, RegistryError>;

    /// Retrieve a listing by ID.
    async fn get_listing(&self, id: ListingId) -> Result<Option<ServiceListing>, RegistryError>;

    /// Search listings matching the given filter.
    async fn search(
        &self,
        filter: SearchFilter,
    ) -> Result<Vec<ServiceListing>, RegistryError>;
}

/// Errors returned by [`ServiceRegistryStore`] operations.
#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("validation error: {0}")]
    Validation(String),

    #[error("internal error: {0}")]
    Internal(String),
}
