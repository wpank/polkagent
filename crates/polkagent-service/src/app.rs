//! Application service — the central object that wires all Polkagent
//! components together.
//!
//! [`AppService`] is built via [`AppServiceBuilder`] which ensures all
//! required components are provided before constructing the service. Once
//! built, `AppService` is the single facade that upstream layers (HTTP API,
//! CLI, TUI) use to drive the platform.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use polkagent_config::Config;
use polkagent_core::{AgentId, AgentSpec, EffectId, RunId, RunState};
use polkagent_event::{EventBus, EventReceiver, EventRecorder};
use polkagent_executor_trait::ModelExecutor;
use polkagent_run::RunManager;
use polkagent_store_trait::{EffectStore, RunStore, RunSummary};
use tracing::{info, instrument, warn};

use crate::error::ServiceError;
use crate::provider::ProviderRegistry;

// ---------------------------------------------------------------------------
// AppServiceBuilder
// ---------------------------------------------------------------------------

/// Builder for [`AppService`].
///
/// All required components must be provided via the `with_*` methods before
/// calling [`build`](AppServiceBuilder::build).
#[derive(Default)]
pub struct AppServiceBuilder {
    config: Option<Config>,
    executor: Option<Arc<dyn ModelExecutor>>,
    run_store: Option<Arc<dyn RunStore>>,
    effect_store: Option<Arc<dyn EffectStore>>,
    event_bus: Option<EventBus>,
    event_recorder: Option<EventRecorder>,
    provider_registry: Option<ProviderRegistry>,
}

impl AppServiceBuilder {
    /// Create a new builder with no components set.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the application configuration.
    #[must_use]
    pub fn with_config(mut self, config: Config) -> Self {
        self.config = Some(config);
        self
    }

    /// Set the default model executor.
    #[must_use]
    pub fn with_executor(mut self, executor: Arc<dyn ModelExecutor>) -> Self {
        self.executor = Some(executor);
        self
    }

    /// Set the run store.
    #[must_use]
    pub fn with_run_store(mut self, store: Arc<dyn RunStore>) -> Self {
        self.run_store = Some(store);
        self
    }

    /// Set the effect store.
    #[must_use]
    pub fn with_effect_store(mut self, store: Arc<dyn EffectStore>) -> Self {
        self.effect_store = Some(store);
        self
    }

    /// Set the event bus.
    #[must_use]
    pub fn with_event_bus(mut self, bus: EventBus) -> Self {
        self.event_bus = Some(bus);
        self
    }

    /// Set the event recorder.
    #[must_use]
    pub fn with_event_recorder(mut self, recorder: EventRecorder) -> Self {
        self.event_recorder = Some(recorder);
        self
    }

    /// Set the provider registry.
    #[must_use]
    pub fn with_provider_registry(mut self, registry: ProviderRegistry) -> Self {
        self.provider_registry = Some(registry);
        self
    }

    /// Build the [`AppService`], consuming the builder.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::NotInitialized`] if any required component is
    /// missing.
    pub fn build(self) -> Result<AppService, ServiceError> {
        let config = self.config.ok_or_else(|| ServiceError::NotInitialized {
            component: "config".into(),
        })?;
        let run_store = self
            .run_store
            .ok_or_else(|| ServiceError::NotInitialized {
                component: "run_store".into(),
            })?;
        let event_bus = self.event_bus.unwrap_or_default();
        let event_recorder = self.event_recorder.ok_or_else(|| {
            ServiceError::NotInitialized {
                component: "event_recorder".into(),
            }
        })?;

        let run_manager = RunManager::new(Arc::clone(&run_store), event_recorder.clone());

        Ok(AppService {
            config,
            executor: self.executor,
            run_store,
            effect_store: self.effect_store,
            event_bus,
            event_recorder,
            run_manager,
            provider_registry: self
                .provider_registry
                .unwrap_or_default(),
            agents: Arc::new(Mutex::new(HashMap::new())),
        })
    }
}

// ---------------------------------------------------------------------------
// AppService
// ---------------------------------------------------------------------------

/// The central application service that wires all Polkagent components
/// together.
///
/// Upstream layers (HTTP API, CLI, TUI) interact with Polkagent exclusively
/// through this facade. It coordinates run lifecycle management, effect
/// approval, event streaming, and provider routing.
///
/// # Thread safety
///
/// `AppService` is `Send + Sync`. It can be wrapped in an `Arc` and shared
/// across Tokio tasks.
pub struct AppService {
    /// The resolved application configuration.
    config: Config,
    /// Default model executor (used when no provider-specific executor is
    /// requested).
    executor: Option<Arc<dyn ModelExecutor>>,
    /// Run persistence store.
    run_store: Arc<dyn RunStore>,
    /// Effect persistence store (optional — effects are disabled if absent).
    effect_store: Option<Arc<dyn EffectStore>>,
    /// In-process event bus for pub/sub.
    event_bus: EventBus,
    /// Event recorder (store + bus).
    #[allow(dead_code)]
    event_recorder: EventRecorder,
    /// Run lifecycle manager.
    run_manager: RunManager,
    /// Provider registry for routing model requests.
    provider_registry: ProviderRegistry,
    /// In-memory agent registry (specs indexed by ID).
    agents: Arc<Mutex<HashMap<AgentId, AgentSpec>>>,
}

impl std::fmt::Debug for AppService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppService")
            .field("config_schema_version", &self.config.meta.schema_version)
            .field("has_executor", &self.executor.is_some())
            .field("has_effect_store", &self.effect_store.is_some())
            .field("provider_count", &self.provider_registry.len())
            .finish()
    }
}

impl AppService {
    /// Return a new [`AppServiceBuilder`].
    #[must_use]
    pub fn builder() -> AppServiceBuilder {
        AppServiceBuilder::new()
    }

    /// Return a reference to the resolved configuration.
    #[must_use]
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Return a reference to the provider registry.
    #[must_use]
    pub fn provider_registry(&self) -> &ProviderRegistry {
        &self.provider_registry
    }

    /// Return a mutable reference to the provider registry.
    pub fn provider_registry_mut(&mut self) -> &mut ProviderRegistry {
        &mut self.provider_registry
    }

    // -----------------------------------------------------------------------
    // Agent management
    // -----------------------------------------------------------------------

    /// Register a new agent specification and return its ID.
    ///
    /// The agent is stored in-memory and is immediately available for
    /// starting runs.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::Config`] if the spec has an empty name.
    #[instrument(skip(self, spec), fields(agent_name = %spec.name))]
    pub fn create_agent(&self, spec: AgentSpec) -> Result<AgentId, ServiceError> {
        if spec.name.is_empty() {
            return Err(ServiceError::Config {
                message: "agent spec must have a non-empty name".into(),
            });
        }

        let agent_id = spec.id;
        let mut agents = self.agents.lock().map_err(|e| ServiceError::Internal {
            message: format!("agent lock poisoned: {e}"),
        })?;
        agents.insert(agent_id, spec);

        info!(%agent_id, "agent created");
        Ok(agent_id)
    }

    // -----------------------------------------------------------------------
    // Run lifecycle
    // -----------------------------------------------------------------------

    /// Create and enqueue a new run for the given agent.
    ///
    /// The run transitions through `Created -> Queued` and is ready for a
    /// worker to claim and start executing.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::AgentNotFound`] if the agent is not registered,
    /// or propagates run manager errors.
    #[instrument(skip(self, prompt), fields(%agent_id))]
    pub async fn start_run(
        &self,
        agent_id: AgentId,
        prompt: &str,
    ) -> Result<RunId, ServiceError> {
        // Verify agent exists.
        {
            let agents = self.agents.lock().map_err(|e| ServiceError::Internal {
                message: format!("agent lock poisoned: {e}"),
            })?;
            if !agents.contains_key(&agent_id) {
                return Err(ServiceError::AgentNotFound { agent_id });
            }
        }

        let _ = prompt; // Prompt will be used when wiring to the execution engine.

        let run_id = self.run_manager.create_run(agent_id).await?;
        self.run_manager.enqueue_run(run_id).await?;

        info!(%run_id, "run started and enqueued");
        Ok(run_id)
    }

    /// Cancel a running or queued run.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::RunNotFound`] if the run does not exist, or
    /// [`ServiceError::InvalidTransition`] if the run is already terminal.
    #[instrument(skip(self), fields(%run_id))]
    pub async fn cancel_run(&self, run_id: RunId) -> Result<(), ServiceError> {
        self.run_manager
            .cancel_run(run_id, "cancelled by user")
            .await?;
        Ok(())
    }

    /// Approve a pending effect, allowing it to proceed.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::NotInitialized`] if no effect store is
    /// configured, or [`ServiceError::EffectNotFound`] if the effect does
    /// not exist.
    #[instrument(skip(self), fields(%effect_id))]
    pub async fn approve_effect(
        &self,
        effect_id: EffectId,
    ) -> Result<(), ServiceError> {
        let store = self.effect_store.as_ref().ok_or_else(|| {
            ServiceError::NotInitialized {
                component: "effect_store".into(),
            }
        })?;

        // Verify the intent exists.
        let _intent = store.get_intent(effect_id).await.map_err(|e| {
            match e {
                polkagent_store_trait::StoreError::NotFound { .. } => {
                    ServiceError::EffectNotFound { effect_id }
                }
                other => ServiceError::Store {
                    message: other.to_string(),
                },
            }
        })?;

        // In a full implementation, we would transition the intent's approval
        // state and notify the waiting run. For now we verify it exists.
        info!(%effect_id, "effect approved");
        Ok(())
    }

    /// Deny a pending effect with the given reason.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::NotInitialized`] if no effect store is
    /// configured, or [`ServiceError::EffectNotFound`] if the effect does
    /// not exist.
    #[instrument(skip(self, reason), fields(%effect_id))]
    pub async fn deny_effect(
        &self,
        effect_id: EffectId,
        reason: &str,
    ) -> Result<(), ServiceError> {
        let store = self.effect_store.as_ref().ok_or_else(|| {
            ServiceError::NotInitialized {
                component: "effect_store".into(),
            }
        })?;

        // Verify the intent exists.
        let _intent = store.get_intent(effect_id).await.map_err(|e| {
            match e {
                polkagent_store_trait::StoreError::NotFound { .. } => {
                    ServiceError::EffectNotFound { effect_id }
                }
                other => ServiceError::Store {
                    message: other.to_string(),
                },
            }
        })?;

        warn!(%effect_id, %reason, "effect denied");
        Ok(())
    }

    /// Get the current state of a run.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::RunNotFound`] if the run does not exist.
    pub async fn get_run_status(
        &self,
        run_id: RunId,
    ) -> Result<RunState, ServiceError> {
        let state = self.run_manager.get_state(run_id).await?;
        Ok(state)
    }

    /// List runs belonging to the given agent.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::Store`] on store failures.
    pub async fn list_runs(
        &self,
        agent_id: AgentId,
    ) -> Result<Vec<RunSummary>, ServiceError> {
        let summaries = self
            .run_store
            .list_by_agent(&agent_id.to_string(), 100, 0)
            .await?;
        Ok(summaries)
    }

    // -----------------------------------------------------------------------
    // Event streaming
    // -----------------------------------------------------------------------

    /// Subscribe to the real-time event stream.
    ///
    /// Returns an [`EventReceiver`] that yields events as they are published.
    /// The receiver will receive all events published after this call.
    #[must_use]
    pub fn subscribe_events(&self) -> EventReceiver {
        self.event_bus.subscribe()
    }

    /// Return a clone of the event bus.
    #[must_use]
    pub fn event_bus(&self) -> EventBus {
        self.event_bus.clone()
    }

    /// Return the default executor, if one is configured.
    #[must_use]
    pub fn executor(&self) -> Option<Arc<dyn ModelExecutor>> {
        self.executor.clone()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use polkagent_executor_trait::{
        ExecutorError, InferenceRequest, InferenceResponse, StreamEvent,
    };
    use polkagent_store_trait::{
        event::{EventFilter, EventStore, EventStoreError, StoredEvent},
        RunStatus, StoreError,
    };
    use std::collections::{HashMap as StdHashMap, HashSet};

    // ── Fake ModelExecutor ───────────────────────────────────────────────

    #[derive(Debug)]
    struct FakeExecutor;

    #[async_trait::async_trait]
    impl ModelExecutor for FakeExecutor {
        async fn complete(
            &self,
            _request: InferenceRequest,
        ) -> Result<InferenceResponse, ExecutorError> {
            Ok(InferenceResponse {
                text: "fake response".into(),
                tool_calls: vec![],
                stop_reason: "end_turn".into(),
                usage: Default::default(),
                provider_request_id: None,
            })
        }

        async fn stream(
            &self,
            _request: InferenceRequest,
        ) -> Result<
            Box<
                dyn futures::Stream<Item = Result<StreamEvent, ExecutorError>>
                    + Send
                    + Unpin,
            >,
            ExecutorError,
        > {
            Err(ExecutorError::Internal {
                message: "not implemented".into(),
            })
        }

        async fn health(&self) -> Result<(), ExecutorError> {
            Ok(())
        }
    }

    // ── Fake RunStore ───────────────────────────────────────────────────

    #[derive(Debug, Default)]
    struct FakeRunStore {
        runs: Mutex<StdHashMap<String, RunSummary>>,
    }

    #[async_trait::async_trait]
    impl RunStore for FakeRunStore {
        async fn create(
            &self,
            run_id: RunId,
            agent_id: &str,
            status: RunStatus,
        ) -> Result<(), StoreError> {
            let mut guard = self.runs.lock().expect("lock");
            let key = run_id.to_string();
            if guard.contains_key(&key) {
                return Err(StoreError::Conflict {
                    resource_type: "Run",
                    id: key,
                });
            }
            guard.insert(
                key,
                RunSummary {
                    id: run_id,
                    agent_id: agent_id.to_owned(),
                    status,
                    created_at: chrono::Utc::now(),
                    completed_at: None,
                },
            );
            Ok(())
        }

        async fn get(&self, run_id: RunId) -> Result<RunSummary, StoreError> {
            let guard = self.runs.lock().expect("lock");
            guard
                .get(&run_id.to_string())
                .cloned()
                .ok_or_else(|| StoreError::NotFound {
                    resource_type: "Run",
                    id: run_id.to_string(),
                })
        }

        async fn update_state(
            &self,
            run_id: RunId,
            new_status: RunStatus,
        ) -> Result<(), StoreError> {
            let mut guard = self.runs.lock().expect("lock");
            guard
                .get_mut(&run_id.to_string())
                .ok_or_else(|| StoreError::NotFound {
                    resource_type: "Run",
                    id: run_id.to_string(),
                })?
                .status = new_status;
            Ok(())
        }

        async fn list_by_agent(
            &self,
            agent_id: &str,
            limit: u32,
            _offset: u32,
        ) -> Result<Vec<RunSummary>, StoreError> {
            let guard = self.runs.lock().expect("lock");
            let mut runs: Vec<RunSummary> = guard
                .values()
                .filter(|r| r.agent_id == agent_id)
                .cloned()
                .collect();
            runs.sort_by(|a, b| b.created_at.cmp(&a.created_at));
            runs.truncate(limit as usize);
            Ok(runs)
        }

        async fn list_by_state(
            &self,
            status: RunStatus,
            limit: u32,
            _offset: u32,
        ) -> Result<Vec<RunSummary>, StoreError> {
            let guard = self.runs.lock().expect("lock");
            let mut runs: Vec<RunSummary> = guard
                .values()
                .filter(|r| r.status == status)
                .cloned()
                .collect();
            runs.sort_by(|a, b| b.created_at.cmp(&a.created_at));
            runs.truncate(limit as usize);
            Ok(runs)
        }
    }

    // ── Fake EventStore ─────────────────────────────────────────────────

    const TERMINAL_TYPES: &[&str] =
        &["run_completed", "run_failed", "run_cancelled", "run_timed_out"];

    #[derive(Debug, Default)]
    struct FakeEventStore {
        durable: Mutex<Vec<StoredEvent>>,
        sequences: Mutex<StdHashMap<String, u64>>,
        terminal: Mutex<HashSet<String>>,
    }

    #[async_trait::async_trait]
    impl EventStore for FakeEventStore {
        async fn append_durable(
            &self,
            mut event: StoredEvent,
        ) -> Result<StoredEvent, EventStoreError> {
            let mut seqs = self.sequences.lock().expect("lock");
            let current = seqs.get(&event.run_id).copied().unwrap_or(0);
            if event.sequence <= current {
                return Err(EventStoreError::NonMonotonicSequence {
                    run_id: event.run_id.clone(),
                    current,
                    proposed: event.sequence,
                });
            }
            seqs.insert(event.run_id.clone(), event.sequence);

            let mut term = self.terminal.lock().expect("lock");
            if TERMINAL_TYPES.contains(&event.event_type.as_str()) {
                if !term.insert(event.run_id.clone()) {
                    return Err(EventStoreError::DuplicateTerminalEvent {
                        run_id: event.run_id.clone(),
                    });
                }
            }

            let mut durable = self.durable.lock().expect("lock");
            event.global_sequence = (durable.len() + 1) as u64;
            durable.push(event.clone());
            Ok(event)
        }

        async fn append_diagnostic(
            &self,
            _event: StoredEvent,
            _expires_at: String,
        ) -> Result<(), EventStoreError> {
            Ok(())
        }

        async fn read_from_cursor(
            &self,
            cursor: u64,
            limit: usize,
        ) -> Result<Vec<StoredEvent>, EventStoreError> {
            let durable = self.durable.lock().expect("lock");
            Ok(durable
                .iter()
                .filter(|e| e.global_sequence > cursor)
                .take(limit)
                .cloned()
                .collect())
        }

        async fn read_run_events(
            &self,
            run_id: RunId,
        ) -> Result<Vec<StoredEvent>, EventStoreError> {
            let durable = self.durable.lock().expect("lock");
            Ok(durable
                .iter()
                .filter(|e| e.run_id == run_id.to_string())
                .cloned()
                .collect())
        }

        async fn query(
            &self,
            filter: EventFilter,
        ) -> Result<Vec<StoredEvent>, EventStoreError> {
            let durable = self.durable.lock().expect("lock");
            Ok(durable
                .iter()
                .filter(|e| {
                    filter
                        .run_id
                        .as_ref()
                        .map_or(true, |rid| e.run_id == rid.to_string())
                })
                .cloned()
                .collect())
        }

        async fn max_sequence(
            &self,
            run_id: RunId,
        ) -> Result<u64, EventStoreError> {
            let seqs = self.sequences.lock().expect("lock");
            Ok(seqs.get(&run_id.to_string()).copied().unwrap_or(0))
        }

        async fn has_terminal_event(
            &self,
            run_id: RunId,
        ) -> Result<bool, EventStoreError> {
            let term = self.terminal.lock().expect("lock");
            Ok(term.contains(&run_id.to_string()))
        }
    }

    // ── Test helpers ────────────────────────────────────────────────────

    fn build_service() -> AppService {
        let run_store: Arc<dyn RunStore> = Arc::new(FakeRunStore::default());
        let event_store: Arc<dyn EventStore> = Arc::new(FakeEventStore::default());
        let bus = EventBus::new(64);
        let recorder = EventRecorder::new(event_store, bus.clone());

        AppService::builder()
            .with_config(Config::default())
            .with_executor(Arc::new(FakeExecutor))
            .with_run_store(run_store)
            .with_event_bus(bus)
            .with_event_recorder(recorder)
            .build()
            .expect("build service")
    }

    fn make_spec(name: &str) -> AgentSpec {
        AgentSpec::new(AgentId::new(), name, "anthropic/claude-opus-4-6")
    }

    // ── Tests ───────────────────────────────────────────────────────────

    #[test]
    fn builder_fails_without_config() {
        let result = AppService::builder().build();
        assert!(matches!(
            result,
            Err(ServiceError::NotInitialized { .. })
        ));
    }

    #[test]
    fn builder_fails_without_event_recorder() {
        let result = AppService::builder()
            .with_config(Config::default())
            .with_run_store(Arc::new(FakeRunStore::default()))
            .build();
        assert!(matches!(
            result,
            Err(ServiceError::NotInitialized { .. })
        ));
    }

    #[test]
    fn create_agent_returns_id() {
        let service = build_service();
        let spec = make_spec("test-agent");
        let agent_id = service.create_agent(spec).expect("create_agent");
        assert_ne!(agent_id.to_string(), "");
    }

    #[test]
    fn create_agent_empty_name_fails() {
        let service = build_service();
        let spec = make_spec("");
        let result = service.create_agent(spec);
        assert!(matches!(result, Err(ServiceError::Config { .. })));
    }

    #[tokio::test]
    async fn start_run_creates_and_enqueues() {
        let service = build_service();
        let spec = make_spec("runner");
        let agent_id = service.create_agent(spec).expect("create_agent");

        let run_id = service
            .start_run(agent_id, "Hello, agent!")
            .await
            .expect("start_run");

        let state = service
            .get_run_status(run_id)
            .await
            .expect("get_run_status");
        assert_eq!(state, RunState::Queued);
    }

    #[tokio::test]
    async fn start_run_unknown_agent_fails() {
        let service = build_service();
        let unknown_id = AgentId::new();
        let result = service.start_run(unknown_id, "test").await;
        assert!(matches!(
            result,
            Err(ServiceError::AgentNotFound { .. })
        ));
    }

    #[tokio::test]
    async fn cancel_run_transitions_to_cancelled() {
        let service = build_service();
        let spec = make_spec("cancel-test");
        let agent_id = service.create_agent(spec).expect("create_agent");

        let run_id = service
            .start_run(agent_id, "run to cancel")
            .await
            .expect("start_run");

        service.cancel_run(run_id).await.expect("cancel_run");

        let state = service
            .get_run_status(run_id)
            .await
            .expect("get_run_status");
        assert!(matches!(state, RunState::Cancelled { .. }));
    }

    #[tokio::test]
    async fn get_run_status_unknown_run_fails() {
        let service = build_service();
        let result = service.get_run_status(RunId::new()).await;
        assert!(matches!(result, Err(ServiceError::RunNotFound { .. })));
    }

    #[tokio::test]
    async fn list_runs_returns_agent_runs() {
        let service = build_service();
        let spec = make_spec("list-test");
        let agent_id = service.create_agent(spec).expect("create_agent");

        service
            .start_run(agent_id, "run-1")
            .await
            .expect("start_run 1");
        service
            .start_run(agent_id, "run-2")
            .await
            .expect("start_run 2");

        let runs = service.list_runs(agent_id).await.expect("list_runs");
        assert_eq!(runs.len(), 2);
    }

    #[test]
    fn subscribe_events_returns_receiver() {
        let service = build_service();
        let _receiver = service.subscribe_events();
        // No panic = success.
    }

    #[test]
    fn debug_format_does_not_panic() {
        let service = build_service();
        let debug = format!("{service:?}");
        assert!(debug.contains("AppService"));
    }

    #[tokio::test]
    async fn approve_effect_without_store_fails() {
        let service = build_service();
        let result = service.approve_effect(EffectId::new()).await;
        assert!(matches!(
            result,
            Err(ServiceError::NotInitialized { .. })
        ));
    }

    #[tokio::test]
    async fn deny_effect_without_store_fails() {
        let service = build_service();
        let result = service
            .deny_effect(EffectId::new(), "test reason")
            .await;
        assert!(matches!(
            result,
            Err(ServiceError::NotInitialized { .. })
        ));
    }

    #[test]
    fn config_accessor_returns_config() {
        let service = build_service();
        assert_eq!(
            service.config().meta.schema_version,
            polkagent_config::CURRENT_SCHEMA_VERSION
        );
    }

    #[test]
    fn executor_accessor_returns_some() {
        let service = build_service();
        assert!(service.executor().is_some());
    }
}
