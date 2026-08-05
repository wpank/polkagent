//! In-memory implementation of [`ServiceRegistryStore`].
//!
//! Suitable for tests and local development without a database dependency.

use async_trait::async_trait;
use chrono::Utc;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::store::{RegistryError, ServiceRegistryStore};
use crate::types::{CreateListingRequest, ListingId, SearchFilter, ServiceListing};

/// In-memory service registry backed by a `Vec` behind a `RwLock`.
#[derive(Debug, Default)]
pub struct InMemoryServiceRegistry {
    listings: RwLock<Vec<ServiceListing>>,
}

impl InMemoryServiceRegistry {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl ServiceRegistryStore for InMemoryServiceRegistry {
    async fn create_listing(
        &self,
        request: CreateListingRequest,
    ) -> Result<ServiceListing, RegistryError> {
        if request.name.trim().is_empty() {
            return Err(RegistryError::Validation(
                "name must not be empty".to_owned(),
            ));
        }

        let now = Utc::now();
        let listing = ServiceListing {
            id: Uuid::now_v7(),
            name: request.name,
            description: request.description,
            author: request.author,
            version: request.version,
            capabilities: request.capabilities,
            tags: request.tags,
            pricing: request.pricing,
            availability: request.availability,
            protocols: request.protocols,
            sla_tier: request.sla_tier,
            api_schema: request.api_schema,
            created_at: now,
            updated_at: now,
        };

        let mut guard = self.listings.write().await;
        guard.push(listing.clone());
        Ok(listing)
    }

    async fn get_listing(&self, id: ListingId) -> Result<Option<ServiceListing>, RegistryError> {
        let guard = self.listings.read().await;
        Ok(guard.iter().find(|l| l.id == id).cloned())
    }

    async fn search(&self, filter: SearchFilter) -> Result<Vec<ServiceListing>, RegistryError> {
        let guard = self.listings.read().await;
        let limit = filter.limit.unwrap_or(50).min(100) as usize;
        let offset = filter.offset.unwrap_or(0) as usize;

        let results: Vec<ServiceListing> = guard
            .iter()
            .filter(|listing| {
                // Free-text query: match against name or description (case-insensitive).
                if let Some(ref q) = filter.query {
                    let q_lower = q.to_lowercase();
                    let name_match = listing.name.to_lowercase().contains(&q_lower);
                    let desc_match = listing.description.to_lowercase().contains(&q_lower);
                    if !name_match && !desc_match {
                        return false;
                    }
                }

                // Capability filter: listing must declare ALL specified capabilities.
                if !filter.capability.is_empty()
                    && !filter
                        .capability
                        .iter()
                        .all(|cap| listing.capabilities.contains(cap))
                {
                    return false;
                }

                // Tag filter: listing must have at least one matching tag.
                if !filter.tag.is_empty() && !filter.tag.iter().any(|t| listing.tags.contains(t)) {
                    return false;
                }

                // Author filter: exact match.
                if let Some(ref author) = filter.author {
                    if listing.author != *author {
                        return false;
                    }
                }

                // Availability filter.
                if let Some(avail) = filter.availability {
                    if listing.availability != avail {
                        return false;
                    }
                }

                true
            })
            .skip(offset)
            .take(limit)
            .cloned()
            .collect();

        Ok(results)
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    reason = "in-memory registry tests fail immediately when required fixture operations or expected listings are absent"
)]
mod tests {
    use super::*;
    use crate::types::{ServiceAvailability, ServicePricing};

    fn sample_request(name: &str) -> CreateListingRequest {
        CreateListingRequest {
            name: name.to_owned(),
            description: format!("{name} service"),
            author: "test-author".to_owned(),
            version: "1.0.0".to_owned(),
            capabilities: vec!["chain.query".to_owned()],
            tags: vec!["governance".to_owned()],
            pricing: ServicePricing {
                model: "free".to_owned(),
                unit: None,
                price: None,
                currency: None,
            },
            availability: ServiceAvailability::Available,
            protocols: vec!["https_json".to_owned()],
            sla_tier: Some("standard".to_owned()),
            api_schema: None,
        }
    }

    #[tokio::test]
    async fn create_and_get() {
        let store = InMemoryServiceRegistry::new();
        let listing = store.create_listing(sample_request("svc-a")).await.unwrap();
        assert_eq!(listing.name, "svc-a");

        let fetched = store.get_listing(listing.id).await.unwrap();
        assert!(fetched.is_some());
        assert_eq!(fetched.unwrap().name, "svc-a");
    }

    #[tokio::test]
    async fn search_by_capability() {
        let store = InMemoryServiceRegistry::new();
        store.create_listing(sample_request("svc-a")).await.unwrap();
        store.create_listing(sample_request("svc-b")).await.unwrap();

        let results = store
            .search(SearchFilter {
                capability: vec!["chain.query".to_owned()],
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(results.len(), 2);
    }

    #[tokio::test]
    async fn search_by_tag() {
        let store = InMemoryServiceRegistry::new();
        store.create_listing(sample_request("svc-a")).await.unwrap();

        let results = store
            .search(SearchFilter {
                tag: vec!["governance".to_owned()],
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(results.len(), 1);

        let results = store
            .search(SearchFilter {
                tag: vec!["unknown".to_owned()],
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn search_by_author() {
        let store = InMemoryServiceRegistry::new();
        store.create_listing(sample_request("svc-a")).await.unwrap();

        let results = store
            .search(SearchFilter {
                author: Some("test-author".to_owned()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(results.len(), 1);

        let results = store
            .search(SearchFilter {
                author: Some("other".to_owned()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn empty_name_rejected() {
        let store = InMemoryServiceRegistry::new();
        let mut req = sample_request("ok");
        req.name = "  ".to_owned();
        let err = store.create_listing(req).await.unwrap_err();
        assert!(matches!(err, RegistryError::Validation(_)));
    }
}
