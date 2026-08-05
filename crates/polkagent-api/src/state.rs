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
use polkagent_audit::AuditStore;
use polkagent_config::Config;
use polkagent_conversation::ConversationStore;
use polkagent_core::{agent::AgentSpec, AgentId};
use polkagent_event::EventBus;
use polkagent_interaction::{InteractionService, InteractionStore};
use polkagent_marketplace::ServiceRegistryStore;
use polkagent_payment::PaymentStore;
use polkagent_skill::manifest::SkillManifest;
use polkagent_store_trait::event::EventStore;
use polkagent_store_trait::{ArtifactStore, EffectStore};
use polkagent_telemetry::{MetricRecorder, PrometheusRegistry};
use thiserror::Error;

use crate::routes::health::HealthState;
use polkagent_tool::ToolSpec;

use crate::run::RunManagerTrait;

// ---------------------------------------------------------------------------
// SkillRegistry trait
// ---------------------------------------------------------------------------

/// Async registry trait for loaded skill manifests.
///
/// The API layer reads skills through this trait. A concrete implementation
/// might load manifests from disk at startup and cache them in memory.
#[async_trait]
pub trait SkillRegistry: Send + Sync {
    /// List all loaded skill manifests.
    async fn list_skills(&self) -> Vec<SkillManifest>;

    /// Look up a single skill by its name.
    async fn get_skill(&self, name: &str) -> Option<SkillManifest>;

    /// Install a skill from a filesystem path pointing to a skill directory
    /// containing a `skill.toml` manifest.
    ///
    /// Returns the loaded manifest on success.
    async fn install_skill(&self, path: &str) -> Result<SkillManifest, String>;

    /// Uninstall a skill by name, removing it from the registry.
    ///
    /// Returns `true` if the skill was found and removed.
    async fn uninstall_skill(&self, name: &str) -> Result<bool, String>;

    /// Update the runtime configuration for a skill by merging key-value
    /// pairs into the skill's existing config map.
    ///
    /// Returns the updated manifest on success.
    async fn update_skill_config(
        &self,
        name: &str,
        config: serde_json::Value,
    ) -> Result<SkillManifest, String>;
}

// ---------------------------------------------------------------------------
// InMemorySkillRegistry — in-process skill registry
// ---------------------------------------------------------------------------

/// In-memory implementation of [`SkillRegistry`] backed by an `RwLock<Vec>`.
///
/// Skills are loaded from the filesystem via [`polkagent_skill::loader::SkillLoader`] and cached in
/// memory. Install/uninstall mutations modify the in-memory cache only; they
/// do not persist across process restarts.
#[derive(Debug)]
pub struct InMemorySkillRegistry {
    skills: RwLock<Vec<SkillManifest>>,
}

impl InMemorySkillRegistry {
    /// Create an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            skills: RwLock::new(Vec::new()),
        }
    }

    /// Create a registry pre-populated with the given manifests.
    #[must_use]
    pub fn with_manifests(manifests: Vec<SkillManifest>) -> Self {
        Self {
            skills: RwLock::new(manifests),
        }
    }
}

impl Default for InMemorySkillRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SkillRegistry for InMemorySkillRegistry {
    async fn list_skills(&self) -> Vec<SkillManifest> {
        self.skills.read().await.clone()
    }

    async fn get_skill(&self, name: &str) -> Option<SkillManifest> {
        self.skills
            .read()
            .await
            .iter()
            .find(|m| m.skill.name == name)
            .cloned()
    }

    async fn install_skill(&self, path: &str) -> Result<SkillManifest, String> {
        let path = std::path::Path::new(path);
        let manifest =
            polkagent_skill::SkillLoader::load_from_dir(path).map_err(|e| e.to_string())?;

        let mut guard = self.skills.write().await;

        // Reject if a skill with the same name is already loaded.
        if guard.iter().any(|m| m.skill.name == manifest.skill.name) {
            return Err(format!(
                "skill '{}' is already installed",
                manifest.skill.name
            ));
        }

        guard.push(manifest.clone());
        Ok(manifest)
    }

    async fn uninstall_skill(&self, name: &str) -> Result<bool, String> {
        let mut guard = self.skills.write().await;
        let before = guard.len();
        guard.retain(|m| m.skill.name != name);
        Ok(guard.len() < before)
    }

    async fn update_skill_config(
        &self,
        name: &str,
        config: serde_json::Value,
    ) -> Result<SkillManifest, String> {
        let obj = config
            .as_object()
            .ok_or_else(|| "config must be a JSON object".to_owned())?;

        let mut guard = self.skills.write().await;
        let manifest = guard
            .iter_mut()
            .find(|m| m.skill.name == name)
            .ok_or_else(|| format!("skill '{name}' not found"))?;

        // Merge each key-value pair into the manifest's config map.
        for (key, value) in obj {
            // Convert serde_json::Value -> toml::Value for storage in the manifest.
            let toml_val = json_value_to_toml(value)
                .ok_or_else(|| format!("config key '{key}' has an unsupported JSON type"))?;
            manifest.config.insert(key.clone(), toml_val);
        }

        Ok(manifest.clone())
    }
}

/// Best-effort conversion from [`serde_json::Value`] to [`toml::Value`].
///
/// Returns `None` for types that TOML does not support (e.g. `null`).
fn json_value_to_toml(v: &serde_json::Value) -> Option<toml::Value> {
    match v {
        serde_json::Value::Bool(b) => Some(toml::Value::Boolean(*b)),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Some(toml::Value::Integer(i))
            } else {
                n.as_f64().map(toml::Value::Float)
            }
        }
        serde_json::Value::String(s) => Some(toml::Value::String(s.clone())),
        serde_json::Value::Array(arr) => {
            let items: Option<Vec<toml::Value>> = arr.iter().map(json_value_to_toml).collect();
            Some(toml::Value::Array(items?))
        }
        serde_json::Value::Object(map) => {
            let mut table = toml::map::Map::new();
            for (k, val) in map {
                table.insert(k.clone(), json_value_to_toml(val)?);
            }
            Some(toml::Value::Table(table))
        }
        serde_json::Value::Null => None,
    }
}

// ---------------------------------------------------------------------------
// ToolRegistryStore trait
// ---------------------------------------------------------------------------

/// Async query trait for the tool registry.
///
/// This provides read-only access to tool specs so the API layer can expose
/// them without owning the mutable `ToolRegistry` from `polkagent-tool`.
#[async_trait]
pub trait ToolRegistryStore: Send + Sync {
    /// List all registered tool specs.
    async fn list_tools(&self) -> Vec<ToolSpec>;

    /// Look up a tool spec by name.
    async fn get_tool(&self, name: &str) -> Option<ToolSpec>;
}

// ---------------------------------------------------------------------------
// AgentStore trait
// ---------------------------------------------------------------------------

/// Errors produced by agent-store implementations.
///
/// The original infallible port could only turn corrupt durable rows and
/// database failures into false "not found" responses. Durable adapters use
/// this type to fail closed and let the HTTP layer distinguish absence from an
/// unavailable or invalid projection.
#[derive(Debug, Error)]
pub enum AgentStoreError {
    /// A caller supplied an invalid agent specification.
    #[error("invalid agent specification: {0}")]
    InvalidInput(String),
    /// A stored agent specification could not be represented by the API DTO.
    #[error("invalid stored agent projection: {0}")]
    InvalidProjection(String),
    /// The requested write conflicts with an existing durable record.
    #[error("agent store conflict: {0}")]
    Conflict(String),
    /// A persistence or runtime-registration operation failed.
    #[error("agent store unavailable: {0}")]
    Internal(String),
}

/// Async storage trait for agent specs.
///
/// Implementations must be `Send + Sync` so that `Arc<dyn AgentStore>` can be
/// shared across Axum handlers. The in-memory implementation is
/// [`InMemoryAgentStore`]; a durable SQLite-backed implementation can be
/// provided by [`crate::durable::RuntimeAgentStore`].
#[async_trait]
pub trait AgentStore: Send + Sync {
    /// Insert or replace an agent spec.
    async fn insert(&self, spec: AgentSpec) -> Result<(), AgentStoreError>;

    /// Retrieve an agent spec by ID, returning `None` if not found.
    async fn get(&self, id: AgentId) -> Result<Option<AgentSpec>, AgentStoreError>;

    /// Remove an agent spec by ID; returns `true` if it existed.
    async fn remove(&self, id: AgentId) -> Result<bool, AgentStoreError>;

    /// Paginate agent specs sorted by `created_at` ascending.
    ///
    /// Returns `(page, has_more)`.
    async fn list_page(
        &self,
        after: Option<AgentId>,
        limit: usize,
    ) -> Result<(Vec<AgentSpec>, bool), AgentStoreError>;

    /// Return the number of stored agents.
    async fn count(&self) -> Result<usize, AgentStoreError>;
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
    async fn insert(&self, spec: AgentSpec) -> Result<(), AgentStoreError> {
        let mut guard = self.agents.write().await;
        guard.insert(spec.id, spec);
        Ok(())
    }

    async fn get(&self, id: AgentId) -> Result<Option<AgentSpec>, AgentStoreError> {
        let guard = self.agents.read().await;
        Ok(guard.get(&id).cloned())
    }

    async fn remove(&self, id: AgentId) -> Result<bool, AgentStoreError> {
        let mut guard = self.agents.write().await;
        Ok(guard.remove(&id).is_some())
    }

    async fn list_page(
        &self,
        after: Option<AgentId>,
        limit: usize,
    ) -> Result<(Vec<AgentSpec>, bool), AgentStoreError> {
        let sorted = self.list_sorted().await;

        let start = match after {
            Some(cursor) => sorted
                .iter()
                .position(|s| s.id == cursor)
                .map_or(0, |p| p + 1),
            None => 0,
        };

        let page: Vec<AgentSpec> = sorted.iter().skip(start).take(limit + 1).cloned().collect();

        let has_more = page.len() > limit;
        let page = page.into_iter().take(limit).collect();
        Ok((page, has_more))
    }

    async fn count(&self) -> Result<usize, AgentStoreError> {
        Ok(self.agents.read().await.len())
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
    /// Metric recorder for counting runs, effects, and active connections.
    pub metrics: Arc<MetricRecorder>,
    /// Prometheus metrics registry for the `/metrics` endpoint.
    pub prometheus: PrometheusRegistry,
    /// Health state for readiness and startup probes.
    pub health_state: Arc<HealthState>,

    // ------------------------------------------------------------------
    // Optional stores — return 501 Not Implemented when None
    // ------------------------------------------------------------------
    /// Durable event store (optional — returns 501 when not configured).
    pub event_store: Option<Arc<dyn EventStore>>,
    /// Content-addressed artifact store (optional — returns 501 when not configured).
    pub artifact_store: Option<Arc<dyn ArtifactStore>>,
    /// Skill registry (optional — returns 501 when not configured).
    pub skill_registry: Option<Arc<dyn SkillRegistry>>,
    /// Tool registry store (optional — returns 501 when not configured).
    pub tool_registry: Option<Arc<dyn ToolRegistryStore>>,
    /// Payment store (optional — returns 501 when not configured).
    pub payment_store: Option<Arc<dyn PaymentStore>>,
    /// Memory store (optional — returns 501 when not configured).
    pub memory_store: Option<Arc<dyn crate::routes::memory::MemoryStore>>,
    /// Audit log store (optional — returns 501 when not configured).
    pub audit_store: Option<Arc<dyn AuditStore>>,
    /// Conversation store (optional — returns 501 when not configured).
    pub conversation_store: Option<Arc<dyn ConversationStore>>,
    /// Service registry store (optional — returns 501 when not configured).
    pub service_registry_store: Option<Arc<dyn ServiceRegistryStore>>,
    /// Headless interaction service used for agent execution endpoints.
    pub interaction_service: Option<Arc<dyn InteractionService>>,
    /// Read-only durable interaction event projection used for finite HTTP
    /// replay until `InteractionService` owns a paged replay method.
    /// Mutation handlers must never use this store directly.
    pub interaction_store: Option<Arc<dyn InteractionStore>>,
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
            metrics: Arc::new(MetricRecorder::new()),
            prometheus: PrometheusRegistry::with_default_metrics(),
            health_state: Arc::new(HealthState::new_ready()),
            event_store: None,
            artifact_store: None,
            skill_registry: None,
            tool_registry: None,
            payment_store: None,
            memory_store: None,
            audit_store: None,
            conversation_store: None,
            service_registry_store: None,
            interaction_service: None,
            interaction_store: None,
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

    /// Set the skill registry.
    #[must_use]
    pub fn with_skill_registry(mut self, registry: Arc<dyn SkillRegistry>) -> Self {
        self.skill_registry = Some(registry);
        self
    }

    /// Set the tool registry store.
    #[must_use]
    pub fn with_tool_registry(mut self, registry: Arc<dyn ToolRegistryStore>) -> Self {
        self.tool_registry = Some(registry);
        self
    }

    /// Set the payment store.
    #[must_use]
    pub fn with_payment_store(mut self, store: Arc<dyn PaymentStore>) -> Self {
        self.payment_store = Some(store);
        self
    }

    /// Set the memory store.
    #[must_use]
    pub fn with_memory_store(mut self, store: Arc<dyn crate::routes::memory::MemoryStore>) -> Self {
        self.memory_store = Some(store);
        self
    }

    /// Set the audit store.
    #[must_use]
    pub fn with_audit_store(mut self, store: Arc<dyn AuditStore>) -> Self {
        self.audit_store = Some(store);
        self
    }

    /// Set the conversation store.
    #[must_use]
    pub fn with_conversation_store(mut self, store: Arc<dyn ConversationStore>) -> Self {
        self.conversation_store = Some(store);
        self
    }

    /// Set the service registry store.
    #[must_use]
    pub fn with_service_registry_store(mut self, store: Arc<dyn ServiceRegistryStore>) -> Self {
        self.service_registry_store = Some(store);
        self
    }

    /// Set the headless interaction execution service.
    #[must_use]
    pub fn with_interaction_service(mut self, service: Arc<dyn InteractionService>) -> Self {
        self.interaction_service = Some(service);
        self
    }

    /// Set the read-only durable interaction event projection.
    #[must_use]
    pub fn with_interaction_store(mut self, store: Arc<dyn InteractionStore>) -> Self {
        self.interaction_store = Some(store);
        self
    }

    /// Set the health state (for readiness/startup probes).
    #[must_use]
    pub fn with_health_state(mut self, state: Arc<HealthState>) -> Self {
        self.health_state = state;
        self
    }

    /// Seconds elapsed since the server started.
    #[must_use]
    pub fn uptime_secs(&self) -> u64 {
        self.started_at.elapsed().as_secs()
    }
}

// ---------------------------------------------------------------------------
// FromRef implementations for sub-state extraction
// ---------------------------------------------------------------------------

impl axum::extract::FromRef<AppState> for Arc<HealthState> {
    fn from_ref(state: &AppState) -> Self {
        Arc::clone(&state.health_state)
    }
}
