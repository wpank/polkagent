//! `polkagent-marketplace` — agent-service listing and discovery registry.
//!
//! This crate defines the core types and trait for the self-hostable
//! marketplace registry (PRD-12 §5.5). Service declarations include
//! capabilities, pricing, availability, and version metadata.
//!
//! # Crate layout
//!
//! | Module | Purpose |
//! |---|---|
//! | [`types`] | Service listing, pricing, and search data types |
//! | [`store`] | `ServiceRegistryStore` async trait |
//! | [`memory`] | In-memory store for tests and local development |

pub mod memory;
pub mod store;
pub mod types;

pub use store::ServiceRegistryStore;
pub use types::{
    CreateListingRequest, SearchFilter, ServiceAvailability, ServiceListing, ServicePricing,
};
