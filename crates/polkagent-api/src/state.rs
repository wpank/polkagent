//! Shared application state threaded through every Axum handler via
//! `axum::extract::State`.
//!
//! `AppState` is an `Arc`-wrapped struct so that cloning the state for each
//! request is cheap (pointer copy only). All inner fields are themselves
//! `Arc`-wrapped or otherwise `Send + Sync`.
//!
//! # Storage abstraction
//!
//! The [`AgentStore`] trait abstracts over agent persistence. The in-memory
//! [`InMemoryAgentStore`] is used for tests and local development. A durable
//! SQLite-backed implementation can be injected in production.

use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use polkagent_config::Config;
use polkagent_core::{AgentId, agent::AgentSpec};
use polkagent_event::EventBus;
use polkagent_store_trait::{ArtifactStore, EffectStore};
use polkagent_store_trait::event::EventStore;

use crate::run::RunManagerTrait;

// ---------------------------------------------------------------------------
// AgentStore trait
// ---------------------------------------------------------------------------

/// Async storage trait for agent specs.
///
/// Implementations must be `Send + Sync` so that `Arc<dyn AgentStore>` can be
/// shared across Axum handlers. The in-memory implementation is
/// [`InMemoryAgentStore`]; a durable SQLite-backed implementation can be
/// provided by the `polkagent-store-sqlite` crate.
#[async_trait]
pub trait AgentStore: Send + Sync {
    /// Insert or replace an agent spec.
    async fn insert(&self, spec: AgentSpec);

    /// Retrieve an agent spec by ID, returning `None` if not found.
    async fn get(&self, id: AgentId) -> Option<AgentSpec>;

    /// Remove an agent spec by ID; returns `true` if it existed.
    async fn remove(&self, id: AgentId) -> bool;

    /// Paginate agent specs sorted by `created_at` ascending.
    ///
    /// Returns `(page, has_more)`.
    async fn list_page(
        &self,
        after: Option<AgentId>,
        limit: usize,
    ) -> (Vec<AgentSpec>, bool);

    /// Return the number of stored agents.
    async fn count(&self) -> usize;
}

// ---------------------------------------------------------------------------
// InMemoryAgentStore — in-process store for AgentSpec
// ---------------------------------------------------------------------------

use std::collections::HashMap;
use tokio::sync::RwLock;

/// Minimal in-process store for agent specs.
///
/// A full implementation would delegate to a durable database. This in-memory
/// version allows the API crate to function without a database dependency in
/// tests and during early development.
#[derive(Debug, Default)]
pub struct InMemoryAgentStore {
    agents: RwLock<HashMap<AgentId, AgentSpec>>,
}

impl InMemoryAgentStore {
    /// Create an empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Return all agent specs, sorted by `created_at` ascending.
    async fn list_sorted(&self) -> Vec<AgentSpec> {
        let guard = self.agents.read().await;
        let mut all: Vec<AgentSpec> = guard.values().cloned().collect();
        all.sort_by_key(|s: &AgentSpec| s.created_at);
        all
    }
}

#[async_trait]
impl AgentStore for InMemoryAgentStore {
    async fn insert(&self, spec: AgentSpec) {
        let mut guard = self.agents.write().await;
        guard.insert(spec.id, spec);
    }

    async fn get(&self, id: AgentId) -> Option<AgentSpec> {
        let guard = self.agents.read().await;
        guard.get(&id).cloned()
    }

    async fn remove(&self, id: AgentId) -> bool {
        let mut guard = self.agents.write().await;
        guard.remove(&id).is_some()
    }

    async fn list_page(
        &self,
        after: Option<AgentId>,
        limit: usize,
    ) -> (Vec<AgentSpec>, bool) {
        let sorted = self.list_sorted().await;

        let start = match after {
            Some(cursor) => sorted
                .iter()
                .position(|s| s.id == cursor)
                .map(|p| p + 1)
                .unwrap_or(0),
            None => 0,
        };

        let page: Vec<AgentSpec> = sorted
            .iter()
            .skip(start)
            .take(limit + 1)
            .cloned()
            .collect();

        let has_more = page.len() > limit;
        let page = page.into_iter().take(limit).collect();
        (page, has_more)
    }

    async fn count(&self) -> usize {
        self.agents.read().await.len()
    }
}

// ---------------------------------------------------------------------------
// AppState
// ---------------------------------------------------------------------------

/// Shared state for every API request.
///
/// Cloned cheaply (all fields are `Arc`-wrapped) for each Axum request handler.
/// Storage backends are injected as trait objects, enabling tests to use
/// in-memory implementations while production uses durable stores.
#[derive(Clone)]
pub struct AppState {
    /// Active platform configuration.
    pub config: Arc<Config>,
    /// Agent spec store (trait object — can be in-memory or durable).
    pub agents: Arc<dyn AgentStore>,
    /// Run lifecycle manager (trait object — can be in-memory or durable).
    pub run_manager: Arc<dyn RunManagerTrait>,
    /// Effect/outbox store (used for database readiness checks).
    pub effect_store: Arc<dyn EffectStore>,
    /// In-process event bus for real-time event streaming (PRD-14 WebSocket).
    pub event_bus: EventBus,
    /// Monotonic clock marking when the server was started.
    pub started_at: Instant,
    /// UTC wall-clock time the server started (for the system/info endpoint).
    pub started_at_utc: DateTime<Utc>,

    // ------------------------------------------------------------------
    // Optional stores — return 501 Not Implemented when None
    // ------------------------------------------------------------------

    /// Durable event store (optional — returns 501 when not configured).
    pub event_store: Option<Arc<dyn EventStore>>,
    /// Content-addressed artifact store (optional — returns 501 when not configured).
    pub artifact_store: Option<Arc<dyn ArtifactStore>>,
}

impl AppState {
    /// Construct a new `AppState` with injectable storage backends.
    ///
    /// # Arguments
    ///
    /// - `config`: the active platform configuration.
    /// - `agents`: the agent spec store (in-memory or durable).
    /// - `run_manager`: manages run lifecycle operations.
    /// - `effect_store`: the durable effect/outbox store (used for readiness checks).
    /// - `event_bus`: the in-process event bus for real-time WebSocket streaming.
    pub fn new(
        config: Config,
        agents: Arc<dyn AgentStore>,
        run_manager: Arc<dyn RunManagerTrait>,
        effect_store: Arc<dyn EffectStore>,
        event_bus: EventBus,
    ) -> Self {
        Self {
            config: Arc::new(config),
            agents,
            run_manager,
            effect_store,
            event_bus,
            started_at: Instant::now(),
            started_at_utc: Utc::now(),
            event_store: None,
            artifact_store: None,
        }
    }

    /// Set the durable event store.
    #[must_use]
    pub fn with_event_store(mut self, store: Arc<dyn EventStore>) -> Self {
        self.event_store = Some(store);
        self
    }

    /// Set the content-addressed artifact store.
    #[must_use]
    pub fn with_artifact_store(mut self, store: Arc<dyn ArtifactStore>) -> Self {
        self.artifact_store = Some(store);
        self
    }

    /// Seconds elapsed since the server started.
    #[must_use]
    pub fn uptime_secs(&self) -> u64 {
        self.started_at.elapsed().as_secs()
    }
}
