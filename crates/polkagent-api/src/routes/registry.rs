//! Agent-service listing and discovery endpoints (PRD-12 §5.5).
//!
//! | Method | Path | Handler |
//! |---|---|---|
//! | `POST` | `/registry/listings` | [`create_listing`] |
//! | `GET` | `/registry/search` | [`search_listings`] |
//! | `GET` | `/registry/listings/:id` | [`get_listing`] |
//!
//! All endpoints return 501 Not Implemented when no
//! [`ServiceRegistryStore`](polkagent_marketplace::ServiceRegistryStore)
//! has been configured in [`AppState`].

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use tracing::instrument;
use uuid::Uuid;

use polkagent_marketplace::store::RegistryError;

use crate::{
    dto::{
        CreateServiceListingRequest, ListServiceListingsResponse, SearchListingsQuery,
        ServiceListingResponse, API_VERSION,
    },
    error::ApiError,
    state::AppState,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn listing_to_response(listing: &polkagent_marketplace::ServiceListing) -> ServiceListingResponse {
    ServiceListingResponse {
        version: API_VERSION.to_owned(),
        id: listing.id.to_string(),
        name: listing.name.clone(),
        description: listing.description.clone(),
        author: listing.author.clone(),
        service_version: listing.version.clone(),
        capabilities: listing.capabilities.clone(),
        tags: listing.tags.clone(),
        pricing: listing.pricing.clone(),
        availability: listing.availability.as_str().to_owned(),
        protocols: listing.protocols.clone(),
        sla_tier: listing.sla_tier.clone(),
        api_schema: listing.api_schema.clone(),
        created_at: listing.created_at.to_rfc3339(),
        updated_at: listing.updated_at.to_rfc3339(),
    }
}

impl From<RegistryError> for ApiError {
    fn from(err: RegistryError) -> Self {
        match err {
            RegistryError::Validation(msg) => ApiError::ValidationError(msg),
            RegistryError::Internal(msg) => ApiError::InternalError(msg),
        }
    }
}

// ---------------------------------------------------------------------------
// POST /registry/listings
// ---------------------------------------------------------------------------

#[instrument(skip(state, body), fields(name = %body.name))]
pub async fn create_listing(
    State(state): State<AppState>,
    Json(body): Json<CreateServiceListingRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let registry = state
        .service_registry_store
        .as_ref()
        .ok_or_else(|| ApiError::NotImplemented("service registry not configured".to_owned()))?;

    let request = polkagent_marketplace::CreateListingRequest {
        name: body.name,
        description: body.description,
        author: body.author,
        version: body.version,
        capabilities: body.capabilities,
        tags: body.tags,
        pricing: body.pricing,
        availability: body.availability,
        protocols: body.protocols,
        sla_tier: body.sla_tier,
        api_schema: body.api_schema,
    };

    let listing = registry.create_listing(request).await?;
    let response = listing_to_response(&listing);
    Ok((StatusCode::CREATED, Json(response)))
}

// ---------------------------------------------------------------------------
// GET /registry/listings/:id
// ---------------------------------------------------------------------------

#[instrument(skip(state), fields(listing_id = %id))]
pub async fn get_listing(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let registry = state
        .service_registry_store
        .as_ref()
        .ok_or_else(|| ApiError::NotImplemented("service registry not configured".to_owned()))?;

    let uuid = Uuid::parse_str(&id)
        .map_err(|_| ApiError::ValidationError(format!("invalid listing id: {id}")))?;

    let listing = registry
        .get_listing(uuid)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("listing '{id}'")))?;

    Ok(Json(listing_to_response(&listing)))
}

// ---------------------------------------------------------------------------
// GET /registry/search
// ---------------------------------------------------------------------------

#[instrument(skip(state))]
pub async fn search_listings(
    State(state): State<AppState>,
    Query(query): Query<SearchListingsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let registry = state
        .service_registry_store
        .as_ref()
        .ok_or_else(|| ApiError::NotImplemented("service registry not configured".to_owned()))?;

    let filter = polkagent_marketplace::SearchFilter {
        query: query.q,
        capability: query
            .capability
            .map(|s| s.split(',').map(|c| c.trim().to_owned()).collect())
            .unwrap_or_default(),
        tag: query
            .tag
            .map(|s| s.split(',').map(|t| t.trim().to_owned()).collect())
            .unwrap_or_default(),
        author: query.author,
        availability: None,
        limit: query.limit,
        offset: query.offset,
    };

    let listings = registry.search(filter).await?;
    let data: Vec<ServiceListingResponse> = listings.iter().map(listing_to_response).collect();

    Ok(Json(ListServiceListingsResponse {
        version: API_VERSION.to_owned(),
        data,
    }))
}
