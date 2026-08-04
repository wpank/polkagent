//! Data types for service listings, pricing, availability, and search filters.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A unique identifier for a service listing.
pub type ListingId = Uuid;

/// Pricing model for a service listing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServicePricing {
    /// Pricing model: `"free"`, `"usage_metered"`, `"subscription"`, `"one_time"`.
    pub model: String,
    /// Unit of metering (e.g. `"per_invocation"`, `"per_token"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// Price per unit as a decimal string (e.g. `"0.001"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub price: Option<String>,
    /// Currency code (e.g. `"USD"`, `"DOT"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,
}

/// Availability status for a service listing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceAvailability {
    Available,
    Maintenance,
    Deprecated,
    Offline,
}

impl ServiceAvailability {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::Maintenance => "maintenance",
            Self::Deprecated => "deprecated",
            Self::Offline => "offline",
        }
    }
}

/// A stored service listing in the registry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceListing {
    /// Unique listing identifier.
    pub id: ListingId,
    /// Human-readable service name.
    pub name: String,
    /// Service description.
    pub description: String,
    /// Author / publisher identifier.
    pub author: String,
    /// Semver version string.
    pub version: String,
    /// Declared capabilities (e.g. `["chain.query", "file.read"]`).
    pub capabilities: Vec<String>,
    /// Discovery tags (e.g. `["governance", "staking", "defi"]`).
    pub tags: Vec<String>,
    /// Pricing information.
    pub pricing: ServicePricing,
    /// Current availability status.
    pub availability: ServiceAvailability,
    /// Supported invocation protocols.
    #[serde(default)]
    pub protocols: Vec<String>,
    /// SLA tier: `"best_effort"`, `"standard"`, `"premium"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sla_tier: Option<String>,
    /// API schema URL or inline JSON schema.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_schema: Option<serde_json::Value>,
    /// When the listing was created.
    pub created_at: DateTime<Utc>,
    /// When the listing was last updated.
    pub updated_at: DateTime<Utc>,
}

/// Request to create a new service listing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateListingRequest {
    /// Human-readable service name.
    pub name: String,
    /// Service description.
    pub description: String,
    /// Author / publisher identifier.
    pub author: String,
    /// Semver version string.
    pub version: String,
    /// Declared capabilities.
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// Discovery tags.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Pricing information.
    pub pricing: ServicePricing,
    /// Current availability status (defaults to `available`).
    #[serde(default = "default_availability")]
    pub availability: ServiceAvailability,
    /// Supported invocation protocols.
    #[serde(default)]
    pub protocols: Vec<String>,
    /// SLA tier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sla_tier: Option<String>,
    /// API schema.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_schema: Option<serde_json::Value>,
}

fn default_availability() -> ServiceAvailability {
    ServiceAvailability::Available
}

/// Filters for searching the service registry.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SearchFilter {
    /// Free-text query matched against name and description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    /// Filter by capability (listing must declare all specified).
    #[serde(default)]
    pub capability: Vec<String>,
    /// Filter by tag (listing must have at least one match).
    #[serde(default)]
    pub tag: Vec<String>,
    /// Filter by author (exact match).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// Filter by availability status.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub availability: Option<ServiceAvailability>,
    /// Maximum number of results (default 50, max 100).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// Offset for pagination.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<u32>,
}
