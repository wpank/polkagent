//! Shared application state threaded through every Axum handler via
//! `axum::extract::State`.
//!
//! `AppState` is an `Arc`-wrapped struct so that cloning the state for each
//! request is cheap (pointer copy only). All inner fields are themselves
//! `Arc`-wrapped or otherwise `Send + Sync`.

use std::sync::Arc;
use std::time::Instant;

use polkagent_config::Config;
use polkagent_store_trait::EffectStore;

use crate::run::RunManager;

// ---------------------------------------------------------------------------
// AgentRegistry — in-process store for AgentSpec
// ---------------------------------------------------------------------------

use chrono::{DateTime, Utc};
use polkagent_core::{AgentId, agent::AgentSpec};
use std::collections::HashMap;
use tokio::sync::RwLock;

/// Minimal in-process store for agent specs.
///
/// A full implementation would delegate to a durable database. This in-memory
/// version allows the API crate to function without a database dependency in
/// tests and during early development.
#[derive(Debug, Default)]
pub struct AgentRegistry {
    agents: RwLock<HashMap<AgentId, AgentSpec>>,
}

impl AgentRegistry {
    /// Create an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert or replace an agent spec.
    pub async fn insert(&self, spec: AgentSpec) {
        let mut guard = self.agents.write().await;
        guard.insert(spec.id, spec);
    }

    /// Retrieve an agent spec by ID.
    pub async fn get(&self, id: AgentId) -> Option<AgentSpec> {
        let guard = self.agents.read().await;
        guard.get(&id).cloned()
    }

    /// Remove an agent spec by ID; returns `true` if it existed.
    pub async fn remove(&self, id: AgentId) -> bool {
        let mut guard = self.agents.write().await;
        guard.remove(&id).is_some()
    }

    /// Return all agent specs, sorted by `created_at` ascending.
    pub async fn list_sorted(&self) -> Vec<AgentSpec> {
        let guard = self.agents.read().await;
        let mut all: Vec<AgentSpec> = guard.values().cloned().collect();
        all.sort_by_key(|s: &AgentSpec| s.created_at);
        all
    }

    /// Paginate the sorted list using an optional cursor ID and page limit.
    ///
    /// Returns `(page, has_more)`.
    pub async fn list_page(
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

    /// Return the number of stored agents.
    pub async fn count(&self) -> usize {
        self.agents.read().await.len()
    }
}

// ---------------------------------------------------------------------------
// AppState
// ---------------------------------------------------------------------------

/// Shared state for every API request.
///
/// Cloned cheaply (all fields are `Arc`-wrapped) for each Axum request handler.
#[derive(Clone)]
pub struct AppState {
    /// Active platform configuration.
    pub config: Arc<Config>,
    /// Agent spec registry (in-process).
    pub agents: Arc<AgentRegistry>,
    /// Run lifecycle manager.
    pub run_manager: Arc<RunManager>,
    /// Effect/outbox store (used for database readiness checks).
    pub effect_store: Arc<dyn EffectStore>,
    /// Monotonic clock marking when the server was started.
    pub started_at: Instant,
    /// UTC wall-clock time the server started (for the system/info endpoint).
    pub started_at_utc: DateTime<Utc>,
}

impl AppState {
    /// Construct a new `AppState`.
    pub fn new(
        config: Config,
        run_manager: RunManager,
        effect_store: Arc<dyn EffectStore>,
    ) -> Self {
        Self {
            config: Arc::new(config),
            agents: Arc::new(AgentRegistry::new()),
            run_manager: Arc::new(run_manager),
            effect_store,
            started_at: Instant::now(),
            started_at_utc: Utc::now(),
        }
    }

    /// Seconds elapsed since the server started.
    #[must_use]
    pub fn uptime_secs(&self) -> u64 {
        self.started_at.elapsed().as_secs()
    }
}
