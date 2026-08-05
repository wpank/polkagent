//! Application service — the central object that wires all Polkagent
//! components together.
//!
//! [`AppService`] is built via [`AppServiceBuilder`] which ensures all
//! required components are provided before constructing the service. Once
//! built, `AppService` is the single facade that upstream layers (HTTP API,
//! CLI, TUI) use to drive the platform.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use polkagent_card::ActionCard;
use polkagent_chain_trait::ChainClient;
use polkagent_config::watch::{AtomicConfig, ConfigWatcher, ReloadPolicy, WatchEventKind};
use polkagent_config::{Config, ConfigLoader};
use polkagent_core::{
    AgentId, AgentSpec, EffectAttemptId, EffectId, EffectOutcomeId, RunId, RunState, Timestamp,
    WorkerId,
};
use polkagent_event::{EventBus, EventReceiver, EventRecorder};
use polkagent_executor_trait::ModelExecutor;
use polkagent_grant::{
    grant::{GrantResolver, ResolverConfig},
    policy::PolicySet,
};
use polkagent_harness_trait::Harness;
use polkagent_memory::{MemoryEntry, MemoryId, MemoryQuery, MemoryStore};
use polkagent_payment::{Amount, CostRecord, PaymentStore, UsageSummary};
use polkagent_run::{RunManager, RunOrchestrator, TimeoutConfig, TimeoutEnforcer};
use polkagent_signer_trait::Signer;
use polkagent_store_trait::{
    EffectStore, RunStore, RunSummary, StoreError, StoredIntent, StoredOutcome,
};
use tokio::sync::broadcast;
use tracing::{debug, error, info, instrument, warn};

use crate::error::ServiceError;
use crate::explain::{ExplainRequest, SignAndSubmitResult};
use crate::provider::ProviderRegistry;

#[allow(
    clippy::cast_precision_loss,
    reason = "CostRecord stores an approximate display/telemetry value as f64; Amount retains the authoritative integer value"
)]
fn approximate_amount_for_cost_record(value: u128) -> f64 {
    value as f64
}

// ---------------------------------------------------------------------------
// NoopEffectStore — a minimal stub used when no real effect store is provided
// ---------------------------------------------------------------------------

/// A no-op [`EffectStore`] used internally when the service has no real
/// effect store configured. Effect intents are accepted but never persisted.
///
/// **Warning:** This stub logs a warning whenever effects are discarded.
/// Prefer providing a real effect store (e.g. `SqliteEffectStore`) to avoid
/// losing effect data.
#[derive(Debug, Default)]
struct NoopEffectStore;

#[async_trait::async_trait]
impl EffectStore for NoopEffectStore {
    async fn propose_intent(&self, _intent: StoredIntent) -> Result<(), StoreError> {
        warn!(
            "NoopEffectStore: discarding proposed effect intent — \
             no real effect store is configured; effects will not be persisted"
        );
        Ok(())
    }

    async fn claim_intent(
        &self,
        _worker_id: WorkerId,
        _lease_duration: Duration,
    ) -> Result<Option<StoredIntent>, StoreError> {
        Ok(None)
    }

    async fn claim_intent_by_id(
        &self,
        intent_id: EffectId,
        _worker_id: WorkerId,
        _lease_duration: Duration,
    ) -> Result<StoredIntent, StoreError> {
        Err(StoreError::NotFound {
            resource_type: "EffectIntent",
            id: intent_id.to_string(),
        })
    }

    async fn release_claim(
        &self,
        _intent_id: EffectId,
        _worker_id: WorkerId,
    ) -> Result<(), StoreError> {
        Ok(())
    }

    async fn get_intent(&self, intent_id: EffectId) -> Result<StoredIntent, StoreError> {
        Err(StoreError::NotFound {
            resource_type: "EffectIntent",
            id: intent_id.to_string(),
        })
    }

    async fn get_by_run(&self, _run_id: RunId) -> Result<Vec<StoredIntent>, StoreError> {
        Ok(vec![])
    }

    async fn expired_leases(&self, _cutoff: Timestamp) -> Result<Vec<StoredIntent>, StoreError> {
        Ok(vec![])
    }

    async fn record_attempt_start(
        &self,
        _attempt_id: EffectAttemptId,
        _intent_id: EffectId,
        _worker_id: WorkerId,
        _payload: serde_json::Value,
    ) -> Result<(), StoreError> {
        warn!(
            "NoopEffectStore: discarding effect attempt start — \
             no real effect store is configured"
        );
        Ok(())
    }

    async fn record_outcome(&self, _outcome: StoredOutcome) -> Result<(), StoreError> {
        warn!(
            "NoopEffectStore: discarding effect outcome — \
             no real effect store is configured; outcome will not be persisted"
        );
        Ok(())
    }

    async fn unconsumed_outcomes(&self, _run_id: RunId) -> Result<Vec<StoredOutcome>, StoreError> {
        Ok(vec![])
    }

    async fn mark_outcomes_consumed(
        &self,
        _outcome_ids: &[EffectOutcomeId],
    ) -> Result<(), StoreError> {
        Ok(())
    }

    async fn update_intent_state(
        &self,
        intent_id: EffectId,
        _new_state: &str,
    ) -> Result<polkagent_store_trait::StoredIntent, StoreError> {
        Err(StoreError::NotFound {
            resource_type: "EffectIntent",
            id: intent_id.to_string(),
        })
    }
}

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
    harness: Option<Arc<dyn Harness>>,
    run_store: Option<Arc<dyn RunStore>>,
    effect_store: Option<Arc<dyn EffectStore>>,
    event_bus: Option<EventBus>,
    event_recorder: Option<EventRecorder>,
    provider_registry: Option<ProviderRegistry>,
    memory_store: Option<Arc<dyn MemoryStore + Send + Sync>>,
    skill_runner: Option<Arc<polkagent_skill::SkillRunner>>,
    tool_registry: Option<Arc<polkagent_tool::ToolRegistry>>,
    conversation_store: Option<Arc<dyn polkagent_conversation::ConversationStore + Send + Sync>>,
    payment_store: Option<Arc<dyn PaymentStore + Send + Sync>>,
    signer: Option<Arc<dyn Signer>>,
    chain_client: Option<Arc<dyn ChainClient>>,
    webhook_dispatcher: Option<crate::webhook::WebhookDispatcher>,
    scheduler: Option<crate::scheduled::ScheduledTaskManager>,
    plugin_manager: Option<crate::plugins::ServicePluginManager>,
    metadata_watcher: Option<crate::metadata_watcher::MetadataDriftWatcher>,
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

    /// Set the harness for delegating runs to an external agent CLI.
    ///
    /// When set, the orchestrator will route runs through this harness
    /// instead of calling the model executor directly.
    #[must_use]
    pub fn with_harness(mut self, harness: Arc<dyn Harness>) -> Self {
        self.harness = Some(harness);
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

    /// Set the memory store.
    #[must_use]
    pub fn with_memory_store(mut self, store: Arc<dyn MemoryStore + Send + Sync>) -> Self {
        self.memory_store = Some(store);
        self
    }

    /// Set the skill runner.
    #[must_use]
    pub fn with_skill_runner(mut self, runner: Arc<polkagent_skill::SkillRunner>) -> Self {
        self.skill_runner = Some(runner);
        self
    }

    /// Set the tool registry.
    #[must_use]
    pub fn with_tool_registry(mut self, registry: Arc<polkagent_tool::ToolRegistry>) -> Self {
        self.tool_registry = Some(registry);
        self
    }

    /// Set the conversation store.
    #[must_use]
    pub fn with_conversation_store(
        mut self,
        store: Arc<dyn polkagent_conversation::ConversationStore + Send + Sync>,
    ) -> Self {
        self.conversation_store = Some(store);
        self
    }

    /// Set the payment store.
    #[must_use]
    pub fn with_payment_store(mut self, store: Arc<dyn PaymentStore + Send + Sync>) -> Self {
        self.payment_store = Some(store);
        self
    }

    /// Set the signer port for the Explain-Before-Sign pipeline.
    ///
    /// When configured, enables [`AppService::explain_and_sign`] to produce
    /// Polkadot transaction signatures through the isolated signing boundary.
    #[must_use]
    pub fn with_signer(mut self, signer: Arc<dyn Signer>) -> Self {
        self.signer = Some(signer);
        self
    }

    /// Set the chain client port for the Explain-Before-Sign pipeline.
    ///
    /// When configured, enables [`AppService::explain_and_sign`] to fetch
    /// metadata, decode calls, submit extrinsics, and watch finality.
    #[must_use]
    pub fn with_chain_client(mut self, chain_client: Arc<dyn ChainClient>) -> Self {
        self.chain_client = Some(chain_client);
        self
    }

    /// Set the webhook dispatcher subsystem.
    ///
    /// When configured, event bus events are automatically dispatched to
    /// registered webhook endpoints via the
    /// [`polkagent_surface_webhook`] delivery engine.
    #[must_use]
    pub fn with_webhook_dispatcher(
        mut self,
        dispatcher: crate::webhook::WebhookDispatcher,
    ) -> Self {
        self.webhook_dispatcher = Some(dispatcher);
        self
    }

    /// Set the scheduled task manager.
    ///
    /// When configured, enables registering recurring or one-shot agent runs
    /// that fire automatically according to their schedule.
    #[must_use]
    pub fn with_scheduler(mut self, scheduler: crate::scheduled::ScheduledTaskManager) -> Self {
        self.scheduler = Some(scheduler);
        self
    }

    /// Set the plugin manager for plugin lifecycle management.
    ///
    /// When configured, enables [`AppService::plugin_manager`] and the
    /// plugin discovery, loading, and enable/disable lifecycle.
    #[must_use]
    pub fn with_plugin_manager(
        mut self,
        plugin_manager: crate::plugins::ServicePluginManager,
    ) -> Self {
        self.plugin_manager = Some(plugin_manager);
        self
    }

    /// Set the metadata drift watcher.
    ///
    /// When configured, periodically checks for runtime metadata drift on
    /// monitored chains and publishes `MetadataDriftDetected` events.
    #[must_use]
    pub fn with_metadata_watcher(
        mut self,
        watcher: crate::metadata_watcher::MetadataDriftWatcher,
    ) -> Self {
        self.metadata_watcher = Some(watcher);
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
        let run_store = self.run_store.ok_or_else(|| ServiceError::NotInitialized {
            component: "run_store".into(),
        })?;
        let event_bus = self.event_bus.unwrap_or_default();
        let event_recorder = self
            .event_recorder
            .ok_or_else(|| ServiceError::NotInitialized {
                component: "event_recorder".into(),
            })?;

        let run_manager = RunManager::new(Arc::clone(&run_store), event_recorder.clone());

        // Build orchestrator when executor OR harness is present.
        let orchestrator = if self.executor.is_some() || self.harness.is_some() {
            let exec: Arc<dyn ModelExecutor> = self.executor.clone().unwrap_or_else(|| {
                // When only a harness is provided, create a minimal fake executor
                // as a placeholder -- the harness will handle all actual execution.
                polkagent_executor_fake::FakeExecutor::new()
            });

            // Use the configured effect store or fall back to the noop stub.
            let effect_store: Arc<dyn EffectStore> =
                self.effect_store.clone().unwrap_or_else(|| {
                    warn!(
                        "no effect store configured — falling back to NoopEffectStore; \
                         effects will be accepted but not persisted"
                    );
                    Arc::new(NoopEffectStore)
                });

            let pipeline = polkagent_effect::EffectPipeline::new(effect_store, WorkerId::new());

            let grant_resolver =
                GrantResolver::new(PolicySet::default(), ResolverConfig::default());

            let mut orch = RunOrchestrator::new(
                Arc::new(run_manager.clone()),
                exec,
                pipeline,
                event_recorder.clone(),
                grant_resolver,
            );
            if let Some(ref harness) = self.harness {
                orch = orch.with_harness(Arc::clone(harness));
            }
            Some(Arc::new(orch))
        } else {
            None
        };

        // Create the approval broadcast channel.
        let (approval_tx, _) = broadcast::channel::<(EffectId, bool)>(256);

        Ok(AppService {
            atomic_config: Arc::new(AtomicConfig::new(config)),
            executor: self.executor,
            harness: self.harness,
            run_store,
            effect_store: self.effect_store,
            event_bus,
            event_recorder,
            run_manager,
            orchestrator,
            provider_registry: self.provider_registry.unwrap_or_default(),
            agents: Arc::new(Mutex::new(HashMap::new())),
            memory_store: self.memory_store,
            skill_runner: self.skill_runner,
            tool_registry: self.tool_registry,
            conversation_store: self.conversation_store,
            payment_store: self.payment_store,
            signer: self.signer,
            chain_client: self.chain_client,
            approval_tx,
            watcher_shutdown: Mutex::new(None),
            webhook_dispatcher: Mutex::new(self.webhook_dispatcher),
            scheduler: self.scheduler,
            plugin_manager: self.plugin_manager,
            metadata_watcher: std::sync::Mutex::new(self.metadata_watcher),
            timeout_enforcer_handle: Mutex::new(None),
        })
    }
}

// ---------------------------------------------------------------------------
// AppService
// ---------------------------------------------------------------------------

/// A caller-identified run durably correlated to an interaction but not yet
/// visible on the lifecycle event bus.
///
/// Only [`AppService::execute_prepared_run`] can begin this work. Dropping the
/// value leaves a recoverable `created` row; callers should use
/// [`AppService::discard_prepared_run`] when a later preparation step fails.
#[derive(Debug, Clone)]
pub struct PreparedRun {
    run_id: RunId,
    conversation_id: polkagent_core::ConversationId,
    agent_spec: AgentSpec,
}

impl PreparedRun {
    /// Return the caller-generated durable run identity.
    pub const fn run_id(&self) -> RunId {
        self.run_id
    }

    /// Return the conversation attached in the run creation write.
    pub const fn conversation_id(&self) -> polkagent_core::ConversationId {
        self.conversation_id
    }
}

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
    /// The resolved application configuration, swappable at runtime via
    /// hot-reload. Readers call [`live_config`](Self::live_config) to get an
    /// `Arc<Config>` snapshot that remains valid even if a reload races.
    atomic_config: Arc<AtomicConfig<Config>>,
    /// Default model executor (used when no provider-specific executor is
    /// requested).
    executor: Option<Arc<dyn ModelExecutor>>,
    /// Harness for delegating runs to external agent CLIs (optional).
    #[allow(dead_code)]
    harness: Option<Arc<dyn Harness>>,
    /// Run persistence store.
    run_store: Arc<dyn RunStore>,
    /// Effect persistence store (optional — effects are disabled if absent).
    effect_store: Option<Arc<dyn EffectStore>>,
    /// In-process event bus for pub/sub.
    event_bus: EventBus,
    /// Event recorder (store + bus).
    event_recorder: EventRecorder,
    /// Run lifecycle manager.
    run_manager: RunManager,
    /// Full orchestrator (present when an executor is configured).
    orchestrator: Option<Arc<RunOrchestrator>>,
    /// Provider registry for routing model requests.
    provider_registry: ProviderRegistry,
    /// In-memory agent registry (specs indexed by ID).
    agents: Arc<Mutex<HashMap<AgentId, AgentSpec>>>,
    /// Memory store (optional).
    memory_store: Option<Arc<dyn MemoryStore + Send + Sync>>,
    /// Skill runner (optional).
    skill_runner: Option<Arc<polkagent_skill::SkillRunner>>,
    /// Tool registry (optional).
    tool_registry: Option<Arc<polkagent_tool::ToolRegistry>>,
    /// Conversation store (optional).
    conversation_store: Option<Arc<dyn polkagent_conversation::ConversationStore + Send + Sync>>,
    /// Payment store (optional).
    payment_store: Option<Arc<dyn PaymentStore + Send + Sync>>,
    /// Signer port for the Explain-Before-Sign pipeline (optional).
    signer: Option<Arc<dyn Signer>>,
    /// Chain client port for the Explain-Before-Sign pipeline (optional).
    chain_client: Option<Arc<dyn ChainClient>>,
    /// Broadcast channel for notifying the orchestrator of approval decisions.
    /// Sends `(effect_id, approved)` tuples.
    approval_tx: broadcast::Sender<(EffectId, bool)>,
    /// Shutdown sender for the config watcher background task. Sending `true`
    /// signals the watcher loop to exit. `None` when no watcher is running.
    watcher_shutdown: Mutex<Option<tokio::sync::watch::Sender<bool>>>,
    /// Webhook dispatcher subsystem (optional). When present, events published
    /// to the event bus are automatically delivered to registered webhook
    /// endpoints.
    webhook_dispatcher: Mutex<Option<crate::webhook::WebhookDispatcher>>,
    /// Scheduled task manager (optional). When present, enables registering
    /// recurring or one-shot agent runs that fire on a schedule.
    scheduler: Option<crate::scheduled::ScheduledTaskManager>,
    /// Plugin manager (optional). When present, enables plugin discovery,
    /// loading, capability validation, and enable/disable lifecycle.
    plugin_manager: Option<crate::plugins::ServicePluginManager>,
    /// Metadata drift watcher (optional). When present, periodically checks
    /// for runtime metadata drift and publishes events.
    metadata_watcher: std::sync::Mutex<Option<crate::metadata_watcher::MetadataDriftWatcher>>,
    /// Join handle for the background timeout enforcer task. When `Some`, a
    /// background task is periodically sweeping active runs and timing out
    /// those that have exceeded the global deadline.
    timeout_enforcer_handle: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl std::fmt::Debug for AppService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let cfg = self.atomic_config.get();
        f.debug_struct("AppService")
            .field("config_schema_version", &cfg.meta.schema_version)
            .field("has_executor", &self.executor.is_some())
            .field("has_effect_store", &self.effect_store.is_some())
            .field("has_orchestrator", &self.orchestrator.is_some())
            .field("has_memory_store", &self.memory_store.is_some())
            .field("has_tool_registry", &self.tool_registry.is_some())
            .field("has_conversation_store", &self.conversation_store.is_some())
            .field("has_payment_store", &self.payment_store.is_some())
            .field("has_signer", &self.signer.is_some())
            .field("has_chain_client", &self.chain_client.is_some())
            .field("provider_count", &self.provider_registry.len())
            .field(
                "has_webhook_dispatcher",
                &self.webhook_dispatcher.lock().is_ok_and(|g| g.is_some()),
            )
            .field("has_scheduler", &self.scheduler.is_some())
            .field(
                "has_metadata_watcher",
                &self.metadata_watcher.lock().is_ok_and(|g| g.is_some()),
            )
            .field("has_timeout_enforcer", &self.has_timeout_enforcer())
            .finish_non_exhaustive()
    }
}

impl AppService {
    /// Return a new [`AppServiceBuilder`].
    #[must_use]
    pub fn builder() -> AppServiceBuilder {
        AppServiceBuilder::new()
    }

    /// Return a snapshot of the current configuration.
    ///
    /// The returned `Arc<Config>` is a point-in-time snapshot. It remains
    /// valid and consistent even if a hot-reload swaps the underlying
    /// config between the time this is called and when the caller uses
    /// the value.
    #[must_use]
    pub fn config(&self) -> Arc<Config> {
        self.atomic_config.get()
    }

    /// Return a clone of the [`AtomicConfig`] handle.
    ///
    /// Useful for subsystems that need to hold a long-lived reference and
    /// read the latest config on each operation.
    #[must_use]
    pub fn atomic_config(&self) -> Arc<AtomicConfig<Config>> {
        Arc::clone(&self.atomic_config)
    }

    /// Start a background config watcher that polls `config_path` for
    /// changes and atomically swaps the live config when a valid update
    /// is detected.
    ///
    /// The watcher runs in a Tokio blocking task and can be stopped via
    /// [`stop_config_watcher`](Self::stop_config_watcher).
    ///
    /// # Arguments
    ///
    /// * `config_path` - Path to the TOML config file to watch.
    /// * `poll_interval` - How often to poll the file for changes.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::ConfigReload`] if a watcher is already
    /// running, or if the config path does not exist.
    pub fn start_config_watcher(
        &self,
        config_path: impl AsRef<Path>,
        poll_interval: Duration,
    ) -> Result<(), ServiceError> {
        let config_path = config_path.as_ref().to_path_buf();

        // Prevent starting multiple watchers.
        {
            let guard = self
                .watcher_shutdown
                .lock()
                .map_err(|e| ServiceError::Internal {
                    message: format!("watcher lock poisoned: {e}"),
                })?;
            if guard.is_some() {
                return Err(ServiceError::ConfigReload {
                    message: "config watcher is already running".into(),
                });
            }
        }

        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

        // Store the shutdown sender so we can stop the watcher later.
        {
            let mut guard = self
                .watcher_shutdown
                .lock()
                .map_err(|e| ServiceError::Internal {
                    message: format!("watcher lock poisoned: {e}"),
                })?;
            *guard = Some(shutdown_tx);
        }

        let atomic = Arc::clone(&self.atomic_config);
        let path_for_log = config_path.clone();

        tokio::task::spawn_blocking(move || {
            Self::watcher_loop(&config_path, poll_interval, &atomic, &shutdown_rx);
        });

        info!(path = %path_for_log.display(), "config watcher started");
        Ok(())
    }

    /// The polling loop executed inside `spawn_blocking`.
    fn watcher_loop(
        config_path: &Path,
        poll_interval: Duration,
        atomic: &AtomicConfig<Config>,
        shutdown_rx: &tokio::sync::watch::Receiver<bool>,
    ) {
        let watcher = ConfigWatcher::new(config_path, poll_interval, ReloadPolicy::Immediate);

        loop {
            // Check for shutdown signal (non-blocking).
            if *shutdown_rx.borrow() {
                info!(path = %config_path.display(), "config watcher stopped");
                return;
            }

            if let Some(event) = watcher.check_for_changes() {
                match event.kind {
                    WatchEventKind::Modified | WatchEventKind::Created => {
                        info!(
                            path = %event.path.display(),
                            kind = %event.kind,
                            "config file change detected, reloading"
                        );

                        // Load and parse the new config.
                        let new_config = match ConfigLoader::new().with_path(config_path).load() {
                            Ok(cfg) => cfg,
                            Err(err) => {
                                error!(
                                    %err,
                                    path = %config_path.display(),
                                    "failed to parse new config, keeping old config"
                                );
                                std::thread::sleep(poll_interval);
                                continue;
                            }
                        };

                        // Validate the new config.
                        if let Err(validation_errors) =
                            polkagent_config::validate::validate(&new_config)
                        {
                            let msgs: Vec<String> = validation_errors
                                .iter()
                                .map(|e| format!("{}: {}", e.field, e.message))
                                .collect();
                            error!(
                                errors = %msgs.join("; "),
                                path = %config_path.display(),
                                "new config failed validation, keeping old config"
                            );
                            std::thread::sleep(poll_interval);
                            continue;
                        }

                        // Atomically swap to the new config.
                        let _old = atomic.swap(new_config);
                        info!(
                            path = %config_path.display(),
                            "config reloaded successfully"
                        );
                    }
                    WatchEventKind::Deleted => {
                        warn!(
                            path = %event.path.display(),
                            "config file deleted, keeping current config"
                        );
                    }
                }
            }

            std::thread::sleep(poll_interval);
        }
    }

    /// Stop the background config watcher, if one is running.
    ///
    /// This is idempotent: calling it when no watcher is running is a no-op.
    pub fn stop_config_watcher(&self) {
        let mut guard = match self.watcher_shutdown.lock() {
            Ok(g) => g,
            Err(e) => {
                warn!("watcher lock poisoned during stop: {e}");
                return;
            }
        };
        if let Some(tx) = guard.take() {
            let _ = tx.send(true);
            info!("config watcher stop signal sent");
        }
    }

    /// Start the metadata drift watcher, if one was configured via the builder.
    ///
    /// This is idempotent: calling it when no watcher is configured, or when
    /// the watcher is already running, is a no-op.
    pub fn start_metadata_watcher(&self) {
        let mut guard = match self.metadata_watcher.lock() {
            Ok(g) => g,
            Err(e) => {
                warn!("metadata watcher lock poisoned: {e}");
                return;
            }
        };
        if let Some(ref mut watcher) = *guard {
            watcher.start();
        }
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

    /// Subscribe to receive approval decision notifications.
    ///
    /// The channel delivers `(EffectId, approved)` tuples whenever
    /// [`approve_effect`] or [`deny_effect`] is called.
    ///
    /// [`approve_effect`]: AppService::approve_effect
    /// [`deny_effect`]: AppService::deny_effect
    #[must_use]
    pub fn subscribe_approvals(&self) -> broadcast::Receiver<(EffectId, bool)> {
        self.approval_tx.subscribe()
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

    /// Remove an agent specification from the live runtime registry.
    ///
    /// Durable surfaces archive or delete their projection separately and use
    /// this method to ensure the runtime cannot continue accepting work for a
    /// removed agent.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::Internal`] if the agent registry lock is
    /// poisoned.
    pub fn remove_agent(&self, agent_id: AgentId) -> Result<bool, ServiceError> {
        let mut agents = self.agents.lock().map_err(|error| ServiceError::Internal {
            message: format!("agent lock poisoned: {error}"),
        })?;
        Ok(agents.remove(&agent_id).is_some())
    }

    // -----------------------------------------------------------------------
    // Run lifecycle
    // -----------------------------------------------------------------------

    /// Create and begin executing a new run for the given agent.
    ///
    /// The run is created, transitioned to `Queued`, and then a tokio task
    /// is spawned to drive it through execution when an executor is
    /// configured. The `RunCreated` and `RunStarted` events are published to
    /// the event bus. `RunCompleted` or `RunFailed` are published when the
    /// background task finishes.
    ///
    /// Returns the run ID immediately. Execution continues in the background.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::AgentNotFound`] if the agent is not registered,
    /// or propagates run manager errors.
    #[instrument(skip(self, prompt), fields(%agent_id))]
    pub async fn start_run(&self, agent_id: AgentId, prompt: &str) -> Result<RunId, ServiceError> {
        // Verify agent exists and clone its spec for the background task.
        let agent_spec = {
            let agents = self.agents.lock().map_err(|e| ServiceError::Internal {
                message: format!("agent lock poisoned: {e}"),
            })?;
            agents
                .get(&agent_id)
                .cloned()
                .ok_or(ServiceError::AgentNotFound { agent_id })?
        };

        // Enforce max_concurrent_runs when the limit is non-zero.
        let max_concurrent = self.atomic_config.get().execution.max_concurrent_runs;
        if max_concurrent > 0 {
            let running_status = polkagent_store_trait::RunStatus::new("running");
            // Use a high limit so we get an accurate count; in practice the
            // number of concurrent runs is expected to be small (≤ limit).
            let active_runs = self
                .run_store
                .list_by_state(running_status, 10_000, 0)
                .await
                .map_err(|e| ServiceError::Store {
                    message: format!("failed to count running runs: {e}"),
                })?;
            let active_count = u32::try_from(active_runs.len()).unwrap_or(u32::MAX);
            if active_count >= max_concurrent {
                return Err(ServiceError::ConcurrentRunLimitReached {
                    active: active_count,
                    limit: max_concurrent,
                });
            }
        }

        // Create the run record in the Created state.
        // RunManager emits RunCreated via the EventRecorder (→ bus).
        let run_id = self.run_manager.create_run(agent_id).await?;

        // Enqueue the run (Created -> Queued).
        // RunManager emits RunQueued via the EventRecorder (→ bus).
        self.run_manager.enqueue_run(run_id).await?;

        // If an orchestrator is configured, spawn a background task to drive
        // the turn loop.
        if let Some(orchestrator) = self.orchestrator.clone() {
            let prompt_owned = prompt.to_owned();
            let run_manager = self.run_manager.clone();

            // NOTE: If the spawned future panics, tokio::spawn will catch it
            // and return a JoinError. The run will be left in a non-terminal
            // state and must be recovered by the startup reaper (see
            // lifecycle::recover_stuck_runs). A CatchUnwind wrapper is not
            // used here because most panics in async code are non-resumable
            // and the reaper provides a safer recovery path.
            tokio::spawn(async move {
                match orchestrator
                    .execute_run(run_id, &agent_spec, &prompt_owned)
                    .await
                {
                    Ok(outcome) => {
                        // The orchestrator already emitted the terminal
                        // RunCompleted / RunFailed event via the EventRecorder
                        // during execute_run. Publishing a second durable
                        // event here would violate the event store's
                        // duplicate-terminal-event invariant.
                        //
                        // We log the outcome for observability; subscribers
                        // that need the terminal event should listen to the
                        // events the orchestrator already published.
                        info!(
                            %run_id,
                            final_state = %outcome.final_state,
                            "orchestrator task finished"
                        );
                    }
                    Err(err) => {
                        // The orchestrator returned an error. It may or may
                        // not have already transitioned the run to a terminal
                        // state (e.g. HarnessValidation errors exit before
                        // any transition). Only emit a RunFailed event when
                        // the run is still non-terminal to avoid duplicates.
                        warn!(%run_id, %err, "orchestrator task failed");
                        let already_terminal = run_manager
                            .get_state(run_id)
                            .await
                            .is_ok_and(|s| s.is_terminal());
                        if already_terminal {
                            debug!(
                                %run_id,
                                "run already terminal; skipping duplicate fail_run"
                            );
                        } else if let Err(fail_err) =
                            run_manager.fail_run(run_id, &err.to_string()).await
                        {
                            error!(
                                %run_id,
                                %fail_err,
                                "failed to transition run to Failed state"
                            );
                        }
                    }
                }
            });
        } else {
            // No executor configured: the run remains in Queued state
            // for an external worker to claim.
            info!(%run_id, "no executor configured; run queued for external worker");
        }

        info!(%run_id, "run started and enqueued");
        Ok(run_id)
    }

    /// Persist a caller-generated run with its conversation correlation but do
    /// not publish a run event or begin execution.
    ///
    /// Interaction services use this boundary to attach durable turn/run links
    /// and replay subscriptions before the earliest lifecycle observation.
    pub async fn prepare_interaction_run(
        &self,
        run_id: RunId,
        agent_id: AgentId,
        conversation_id: polkagent_core::ConversationId,
    ) -> Result<PreparedRun, ServiceError> {
        let agent_spec = {
            let agents = self.agents.lock().map_err(|error| ServiceError::Internal {
                message: format!("agent lock poisoned: {error}"),
            })?;
            agents
                .get(&agent_id)
                .cloned()
                .ok_or(ServiceError::AgentNotFound { agent_id })?
        };

        let max_concurrent = self.atomic_config.get().execution.max_concurrent_runs;
        if max_concurrent > 0 {
            let active_runs = self
                .run_store
                .list_by_state(polkagent_store_trait::RunStatus::new("running"), 10_000, 0)
                .await
                .map_err(|error| ServiceError::Store {
                    message: format!("failed to count running runs: {error}"),
                })?;
            let active_count = u32::try_from(active_runs.len()).unwrap_or(u32::MAX);
            if active_count >= max_concurrent {
                return Err(ServiceError::ConcurrentRunLimitReached {
                    active: active_count,
                    limit: max_concurrent,
                });
            }
        }

        self.run_manager
            .prepare_run(run_id, agent_id, conversation_id)
            .await?;
        Ok(PreparedRun {
            run_id,
            conversation_id,
            agent_spec,
        })
    }

    /// Publish the first run event, enqueue, and begin a prepared run.
    pub async fn execute_prepared_run(
        &self,
        prepared: PreparedRun,
        prompt: &str,
    ) -> Result<RunId, ServiceError> {
        let run_id = prepared.run_id;
        self.run_manager.activate_prepared_run(run_id).await?;

        if let Some(orchestrator) = self.orchestrator.clone() {
            let prompt = prompt.to_owned();
            let run_manager = self.run_manager.clone();
            tokio::spawn(async move {
                match orchestrator
                    .execute_run(run_id, &prepared.agent_spec, &prompt)
                    .await
                {
                    Ok(outcome) => {
                        info!(
                            %run_id,
                            final_state = %outcome.final_state,
                            "prepared orchestrator task finished"
                        );
                    }
                    Err(error) => {
                        warn!(%run_id, %error, "prepared orchestrator task failed");
                        let already_terminal = run_manager
                            .get_state(run_id)
                            .await
                            .is_ok_and(|state| state.is_terminal());
                        if !already_terminal {
                            if let Err(fail_error) =
                                run_manager.fail_run(run_id, &error.to_string()).await
                            {
                                error!(
                                    %run_id,
                                    %fail_error,
                                    "failed to transition prepared run to Failed state"
                                );
                            }
                        }
                    }
                }
            });
        } else {
            info!(%run_id, "no executor configured; prepared run queued for external worker");
        }

        Ok(run_id)
    }

    /// Delete a prepared run before it has published a lifecycle event.
    pub async fn discard_prepared_run(&self, run_id: RunId) -> Result<(), ServiceError> {
        self.run_manager.discard_prepared_run(run_id).await?;
        Ok(())
    }

    /// Durably fail a prepared run that is already linked to an interaction.
    pub async fn fail_prepared_run(&self, run_id: RunId, reason: &str) -> Result<(), ServiceError> {
        self.run_manager.fail_prepared_run(run_id, reason).await?;
        Ok(())
    }

    /// Terminalize non-terminal linked work after activation or execution
    /// setup fails. Existing terminal state is an idempotent success.
    pub async fn terminalize_run_failure(
        &self,
        run_id: RunId,
        reason: &str,
    ) -> Result<(), ServiceError> {
        if self.run_manager.get_state(run_id).await?.is_terminal() {
            return Ok(());
        }
        self.run_manager.fail_run(run_id, reason).await?;
        Ok(())
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

    /// Pause a running run at an approval checkpoint.
    ///
    /// This is the service-level lifecycle entry point used by control-plane
    /// adapters. It preserves the run state machine and records the matching
    /// approval event rather than updating the durable row directly.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::RunNotFound`] when the run does not exist, or
    /// [`ServiceError::InvalidTransition`] unless it is currently running.
    pub async fn pause_run(&self, run_id: RunId, request_id: &str) -> Result<(), ServiceError> {
        self.run_manager
            .request_approval(run_id, request_id)
            .await?;
        Ok(())
    }

    /// Resume a run that is waiting for approval.
    ///
    /// The approval event receives a stable identifier derived from the
    /// pending request because the current HTTP resume DTO carries no explicit
    /// approval identifier.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::RunNotFound`] when the run does not exist, or
    /// [`ServiceError::InvalidTransition`] unless it is awaiting approval.
    pub async fn resume_run(&self, run_id: RunId) -> Result<(), ServiceError> {
        let state = self.run_manager.get_state(run_id).await?;
        let RunState::AwaitingApproval { request_id } = state else {
            return Err(ServiceError::InvalidTransition {
                message: format!(
                    "can only resume a run in AwaitingApproval state, current state is {state:?}"
                ),
            });
        };
        let approval_id = format!("api-resume:{request_id}");
        self.run_manager
            .grant_approval(run_id, &approval_id)
            .await?;
        Ok(())
    }

    /// Time out a running or queued run.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::RunNotFound`] if the run does not exist, or
    /// [`ServiceError::InvalidTransition`] if the run is already terminal.
    #[instrument(skip(self), fields(%run_id))]
    pub async fn timeout_run(&self, run_id: RunId) -> Result<(), ServiceError> {
        self.run_manager.timeout_run(run_id).await?;
        Ok(())
    }

    /// Approve a pending effect, allowing it to proceed.
    ///
    /// Transitions the effect intent's approval state and broadcasts
    /// `(effect_id, true)` to all approval subscribers.
    ///
    /// Returns the [`ActionCard`] attached to the intent if one was generated
    /// during the turn loop.  The card is `None` for read-only effects or for
    /// intents produced before card generation was introduced.
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
    ) -> Result<Option<ActionCard>, ServiceError> {
        let store = self
            .effect_store
            .as_ref()
            .ok_or_else(|| ServiceError::NotInitialized {
                component: "effect_store".into(),
            })?;

        // Fetch the intent; also extract the action card if present.
        let intent = store.get_intent(effect_id).await.map_err(|e| match e {
            polkagent_store_trait::StoreError::NotFound { .. } => {
                ServiceError::EffectNotFound { effect_id }
            }
            other => ServiceError::Store {
                message: other.to_string(),
            },
        })?;

        // Extract the action card from the stored payload JSON, if present.
        // The field is stored as `action_card` inside the payload blob.
        let action_card: Option<ActionCard> = intent
            .payload
            .get("action_card")
            .and_then(|v| serde_json::from_value(v.clone()).ok());

        // Notify the orchestrator and any other subscribers.
        let _ = self.approval_tx.send((effect_id, true));

        info!(%effect_id, has_card = action_card.is_some(), "effect approved");
        Ok(action_card)
    }

    /// Deny a pending effect with the given reason.
    ///
    /// Transitions the effect intent's approval state and broadcasts
    /// `(effect_id, false)` to all approval subscribers.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::NotInitialized`] if no effect store is
    /// configured, or [`ServiceError::EffectNotFound`] if the effect does
    /// not exist.
    #[instrument(skip(self, reason), fields(%effect_id))]
    pub async fn deny_effect(&self, effect_id: EffectId, reason: &str) -> Result<(), ServiceError> {
        let store = self
            .effect_store
            .as_ref()
            .ok_or_else(|| ServiceError::NotInitialized {
                component: "effect_store".into(),
            })?;

        // Verify the intent exists.
        let _intent = store.get_intent(effect_id).await.map_err(|e| match e {
            polkagent_store_trait::StoreError::NotFound { .. } => {
                ServiceError::EffectNotFound { effect_id }
            }
            other => ServiceError::Store {
                message: other.to_string(),
            },
        })?;

        // Notify the orchestrator and any other subscribers.
        let _ = self.approval_tx.send((effect_id, false));

        warn!(%effect_id, %reason, "effect denied");
        Ok(())
    }

    /// Get the current state of a run.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::RunNotFound`] if the run does not exist.
    pub async fn get_run_status(&self, run_id: RunId) -> Result<RunState, ServiceError> {
        let state = self.run_manager.get_state(run_id).await?;
        Ok(state)
    }

    /// List runs belonging to the given agent.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::Store`] on store failures.
    pub async fn list_runs(&self, agent_id: AgentId) -> Result<Vec<RunSummary>, ServiceError> {
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

    /// Return a clone of the event recorder.
    ///
    /// Callers that need to record additional events (e.g. tool results, audit
    /// trails) can use the recorder directly.
    #[must_use]
    pub fn event_recorder(&self) -> EventRecorder {
        self.event_recorder.clone()
    }

    /// Return the default executor, if one is configured.
    #[must_use]
    pub fn executor(&self) -> Option<Arc<dyn ModelExecutor>> {
        self.executor.clone()
    }

    // -----------------------------------------------------------------------
    // Memory operations
    // -----------------------------------------------------------------------

    /// Store a memory entry for the given agent.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::NotInitialized`] if no memory store is
    /// configured, or [`ServiceError::Store`] on persistence failure.
    pub async fn store_memory(
        &self,
        _agent_id: &AgentId,
        entry: MemoryEntry,
    ) -> Result<MemoryId, ServiceError> {
        let store = self
            .memory_store
            .as_ref()
            .ok_or_else(|| ServiceError::NotInitialized {
                component: "memory_store".into(),
            })?;
        store
            .store_memory(&entry)
            .await
            .map_err(|e| ServiceError::Store {
                message: e.to_string(),
            })
    }

    /// Search memories for the given agent matching the query string.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::NotInitialized`] if no memory store is
    /// configured, or [`ServiceError::Store`] on search failure.
    pub async fn search_memory(
        &self,
        agent_id: &AgentId,
        query: &str,
        limit: usize,
    ) -> Result<Vec<MemoryEntry>, ServiceError> {
        let store = self
            .memory_store
            .as_ref()
            .ok_or_else(|| ServiceError::NotInitialized {
                component: "memory_store".into(),
            })?;
        let memory_query = MemoryQuery {
            agent_id: Some(*agent_id),
            query_text: query.to_owned(),
            memory_types: None,
            limit,
            min_relevance: None,
            since: None,
            episode_id: None,
        };
        store
            .search(&memory_query)
            .await
            .map_err(|e| ServiceError::Store {
                message: e.to_string(),
            })
    }

    // -----------------------------------------------------------------------
    // Payment tracking
    // -----------------------------------------------------------------------

    /// Record an LLM cost entry for the given run.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::NotInitialized`] if no payment store is
    /// configured, or [`ServiceError::Store`] on persistence failure.
    pub async fn record_cost(&self, run_id: &RunId, amount: &Amount) -> Result<(), ServiceError> {
        let store = self
            .payment_store
            .as_ref()
            .ok_or_else(|| ServiceError::NotInitialized {
                component: "payment_store".into(),
            })?;
        let record = CostRecord {
            run_id: run_id.to_string(),
            provider: "unknown".to_owned(),
            model: "unknown".to_owned(),
            input_tokens: 0,
            output_tokens: 0,
            estimated_usd: approximate_amount_for_cost_record(amount.value),
            recorded_at: chrono::Utc::now(),
        };
        store
            .record_cost(record)
            .await
            .map_err(|e| ServiceError::Store {
                message: e.to_string(),
            })
    }

    /// Get aggregated usage statistics for the given agent.
    ///
    /// Returns usage over the last 30 days by default.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::NotInitialized`] if no payment store is
    /// configured, or [`ServiceError::Store`] on retrieval failure.
    pub async fn get_usage(&self, agent_id: &AgentId) -> Result<UsageSummary, ServiceError> {
        let store = self
            .payment_store
            .as_ref()
            .ok_or_else(|| ServiceError::NotInitialized {
                component: "payment_store".into(),
            })?;
        let now = chrono::Utc::now();
        let since = now - chrono::Duration::days(30);
        store
            .get_usage(&agent_id.to_string(), since, now)
            .await
            .map_err(|e| ServiceError::Store {
                message: e.to_string(),
            })
    }

    // -----------------------------------------------------------------------
    // Explain-Before-Sign pipeline
    // -----------------------------------------------------------------------

    /// Run the full Explain-Before-Sign pipeline.
    ///
    /// Decodes the SCALE-encoded call bytes against pinned metadata, builds a
    /// human-readable action card, signs the EXACT original bytes, submits the
    /// signed extrinsic, and watches for finality.
    ///
    /// # Acceptance criteria
    ///
    /// - **AC-P2-001:** Call decoded correctly against pinned metadata.
    /// - **AC-P2-003:** Signer receives exact bytes, never model-modified data.
    /// - **AC-P2-004:** Stale metadata produces explicit error before signing.
    /// - **AC-P2-005:** Wrong-network extrinsic rejected before signing.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::NotInitialized`] if either the signer or chain
    /// client is not configured. Returns [`ServiceError::ExplainPipeline`] if
    /// any stage of the pipeline fails.
    #[instrument(skip(self, request), fields(chain_profile = %request.chain_profile, run_id = %request.run_id))]
    pub async fn explain_and_sign(
        &self,
        request: ExplainRequest,
    ) -> Result<SignAndSubmitResult, ServiceError> {
        let signer = self
            .signer
            .as_ref()
            .ok_or_else(|| ServiceError::NotInitialized {
                component: "signer".into(),
            })?;
        let chain_client =
            self.chain_client
                .as_ref()
                .ok_or_else(|| ServiceError::NotInitialized {
                    component: "chain_client".into(),
                })?;

        let result =
            crate::explain::full_pipeline(chain_client.as_ref(), signer.as_ref(), request).await?;

        Ok(result)
    }

    /// Return a reference to the signer, if configured.
    #[must_use]
    pub fn signer(&self) -> Option<&Arc<dyn Signer>> {
        self.signer.as_ref()
    }

    /// Return a reference to the chain client, if configured.
    #[must_use]
    pub fn chain_client(&self) -> Option<&Arc<dyn ChainClient>> {
        self.chain_client.as_ref()
    }

    // -----------------------------------------------------------------------
    // Optional subsystem accessors
    // -----------------------------------------------------------------------

    /// Return a reference to the skill runner, if configured.
    #[must_use]
    pub fn skill_runner(&self) -> Option<&Arc<polkagent_skill::SkillRunner>> {
        self.skill_runner.as_ref()
    }

    /// Return a reference to the tool registry, if configured.
    #[must_use]
    pub fn tool_registry(&self) -> Option<&Arc<polkagent_tool::ToolRegistry>> {
        self.tool_registry.as_ref()
    }

    /// Return a reference to the conversation store, if configured.
    #[must_use]
    pub fn conversation_store(
        &self,
    ) -> Option<&Arc<dyn polkagent_conversation::ConversationStore + Send + Sync>> {
        self.conversation_store.as_ref()
    }

    /// Return a reference to the memory store, if configured.
    #[must_use]
    pub fn memory_store(&self) -> Option<&Arc<dyn MemoryStore + Send + Sync>> {
        self.memory_store.as_ref()
    }

    /// Return a reference to the payment store, if configured.
    #[must_use]
    pub fn payment_store(&self) -> Option<&Arc<dyn PaymentStore + Send + Sync>> {
        self.payment_store.as_ref()
    }

    /// Return a reference to the plugin manager, if configured.
    #[must_use]
    pub fn plugin_manager(&self) -> Option<&crate::plugins::ServicePluginManager> {
        self.plugin_manager.as_ref()
    }

    /// Start the webhook dispatcher if one was configured via the builder.
    ///
    /// This subscribes to the event bus and begins dispatching matching
    /// events to registered webhook endpoints. The dispatcher runs in the
    /// background until the service is shut down.
    ///
    /// This is a no-op if no dispatcher was configured or if it has already
    /// been started.
    pub fn start_webhook_dispatcher(&self) {
        if let Ok(mut guard) = self.webhook_dispatcher.lock() {
            if let Some(dispatcher) = guard.as_mut() {
                dispatcher.start();
            }
        }
    }

    /// Return `true` if a webhook dispatcher is configured.
    #[must_use]
    pub fn has_webhook_dispatcher(&self) -> bool {
        self.webhook_dispatcher.lock().is_ok_and(|g| g.is_some())
    }

    /// Return a reference to the scheduled task manager, if configured.
    #[must_use]
    pub fn scheduler(&self) -> Option<&crate::scheduled::ScheduledTaskManager> {
        self.scheduler.as_ref()
    }

    /// Start a background task that periodically sweeps active runs and
    /// transitions any that have exceeded the global timeout to `TimedOut`.
    ///
    /// The enforcer runs every `interval` and checks all runs in non-terminal
    /// states (`running`, `queued`, `awaiting_approval`, `waiting_effect`).
    /// Each run is checked via [`TimeoutEnforcer::check`] which evaluates
    /// both per-run deadlines and the global max duration. Runs that have
    /// exceeded their deadline are transitioned to `TimedOut` via
    /// [`RunManager::timeout_run`].
    ///
    /// The join handle is stored internally and can be checked via
    /// [`has_timeout_enforcer`](Self::has_timeout_enforcer).
    pub fn start_timeout_enforcer(&self, config: TimeoutConfig, interval: Duration) {
        let run_store = Arc::clone(&self.run_store);
        let run_manager = self.run_manager.clone();
        info!(
            global_max_secs = config.global_max_duration.map(|d| d.as_secs()),
            interval_secs = interval.as_secs(),
            "timeout enforcer started"
        );
        let enforcer = TimeoutEnforcer::new(config);

        let handle = tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            // The first tick fires immediately; consume it so we start after
            // one full interval.
            ticker.tick().await;

            loop {
                ticker.tick().await;

                let active_states = ["running", "queued", "awaiting_approval", "waiting_effect"];
                for state_str in &active_states {
                    let summaries = match run_store
                        .list_by_state(polkagent_store_trait::RunStatus::new(*state_str), 500, 0)
                        .await
                    {
                        Ok(s) => s,
                        Err(e) => {
                            warn!(state = state_str, %e, "timeout sweep: failed to list runs");
                            continue;
                        }
                    };

                    for summary in summaries {
                        // Build a lightweight Run with only the fields the
                        // enforcer inspects (id, started_at, deadline).
                        let mut run = polkagent_core::run::Run::new(
                            summary.id,
                            polkagent_core::AgentId::new(),
                        );
                        run.started_at = summary.started_at;
                        run.deadline = summary.deadline_at;

                        if let Err(polkagent_run::RunError::DeadlineExceeded(_)) =
                            enforcer.check(&run)
                        {
                            info!(
                                run_id = %run.id,
                                "timeout enforcer: transitioning run to TimedOut"
                            );
                            if let Err(e) = run_manager.timeout_run(run.id).await {
                                warn!(
                                    %e,
                                    "timeout enforcer: failed to transition run"
                                );
                            }
                        }
                    }
                }
            }
        });

        if let Ok(mut guard) = self.timeout_enforcer_handle.lock() {
            *guard = Some(handle);
        }
    }

    /// Return `true` if the timeout enforcer background task is running.
    #[must_use]
    pub fn has_timeout_enforcer(&self) -> bool {
        self.timeout_enforcer_handle
            .lock()
            .is_ok_and(|g| g.is_some())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use polkagent_conversation::{
        types::{Conversation, ConversationSummary, Message},
        ConversationError, ConversationResult, ConversationStore,
    };
    use polkagent_core::event::EventKind;
    use polkagent_core::ids::ConversationId;
    use polkagent_executor_trait::{
        ExecutorError, InferenceRequest, InferenceResponse, StreamEvent, TokenUsage,
    };
    use polkagent_memory::{
        types::{Episode, EpisodeId, MemoryEntry, MemoryId, MemoryQuery, MemoryType},
        MemoryError, MemoryResult,
    };
    use polkagent_payment::{
        CostRecord, PaymentError, PaymentIntent, PaymentReceipt, PaymentStatus, UsageSummary,
    };
    use polkagent_store_trait::{
        event::{EventFilter, EventStore, EventStoreError, StoredEvent},
        RunStatus, StoreError,
    };
    use std::collections::{HashMap as StdHashMap, HashSet};
    use uuid::Uuid;

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
                usage: TokenUsage::default(),
                provider_request_id: None,
            })
        }

        async fn stream(
            &self,
            _request: InferenceRequest,
        ) -> Result<
            Box<dyn futures::Stream<Item = Result<StreamEvent, ExecutorError>> + Send + Unpin>,
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
                    created_at: Utc::now(),
                    started_at: None,
                    completed_at: None,
                    deadline_at: None,
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
            runs.sort_by_key(|run| std::cmp::Reverse(run.created_at));
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
            runs.sort_by_key(|run| std::cmp::Reverse(run.created_at));
            runs.truncate(limit as usize);
            Ok(runs)
        }

        async fn insert_turn(
            &self,
            _turn_id: polkagent_core::TurnId,
            _run_id: RunId,
            _sequence: u32,
            _role: &str,
            _started_at: &str,
            _completed_at: Option<&str>,
            _input_tokens: u32,
            _output_tokens: u32,
        ) -> Result<(), StoreError> {
            Ok(())
        }
    }

    // ── Fake EventStore ─────────────────────────────────────────────────

    const TERMINAL_TYPES: &[&str] = &[
        "run_completed",
        "run_failed",
        "run_cancelled",
        "run_timed_out",
    ];

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
            if TERMINAL_TYPES.contains(&event.event_type.as_str())
                && !term.insert(event.run_id.clone())
            {
                return Err(EventStoreError::DuplicateTerminalEvent {
                    run_id: event.run_id.clone(),
                });
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

        async fn query(&self, filter: EventFilter) -> Result<Vec<StoredEvent>, EventStoreError> {
            let durable = self.durable.lock().expect("lock");
            Ok(durable
                .iter()
                .filter(|e| {
                    filter
                        .run_id
                        .as_ref()
                        .is_none_or(|rid| e.run_id == rid.to_string())
                })
                .cloned()
                .collect())
        }

        async fn max_sequence(&self, run_id: RunId) -> Result<u64, EventStoreError> {
            let seqs = self.sequences.lock().expect("lock");
            Ok(seqs.get(&run_id.to_string()).copied().unwrap_or(0))
        }

        async fn has_terminal_event(&self, run_id: RunId) -> Result<bool, EventStoreError> {
            let term = self.terminal.lock().expect("lock");
            Ok(term.contains(&run_id.to_string()))
        }
    }

    // ── Fake MemoryStore ─────────────────────────────────────────────────

    #[derive(Debug, Default)]
    struct FakeMemoryStore {
        entries: Mutex<StdHashMap<String, MemoryEntry>>,
    }

    #[async_trait::async_trait]
    impl MemoryStore for FakeMemoryStore {
        async fn store_memory(&self, entry: &MemoryEntry) -> MemoryResult<MemoryId> {
            let id = entry.id;
            self.entries
                .lock()
                .expect("lock")
                .insert(id.to_string(), entry.clone());
            Ok(id)
        }

        async fn get_memory(&self, id: MemoryId) -> MemoryResult<MemoryEntry> {
            self.entries
                .lock()
                .expect("lock")
                .get(&id.to_string())
                .cloned()
                .ok_or_else(|| MemoryError::NotFound(id.to_string()))
        }

        async fn search(&self, query: &MemoryQuery) -> MemoryResult<Vec<MemoryEntry>> {
            let guard = self.entries.lock().expect("lock");
            let results: Vec<MemoryEntry> = guard
                .values()
                .filter(|e| {
                    query.agent_id.is_none_or(|aid| e.agent_id == aid)
                        && e.content.contains(&query.query_text)
                })
                .take(query.limit)
                .cloned()
                .collect();
            Ok(results)
        }

        async fn search_with_classification(
            &self,
            query: &MemoryQuery,
            max_classification: polkagent_memory::classification::Classification,
        ) -> MemoryResult<Vec<MemoryEntry>> {
            let guard = self.entries.lock().expect("lock");
            let results: Vec<MemoryEntry> = guard
                .values()
                .filter(|e| {
                    query.agent_id.is_none_or(|aid| e.agent_id == aid)
                        && e.content.contains(&query.query_text)
                        && e.classification <= max_classification
                })
                .take(query.limit)
                .cloned()
                .collect();
            Ok(results)
        }

        async fn list_entries(
            &self,
            _agent_id: &AgentId,
            _limit: usize,
            _offset: usize,
        ) -> MemoryResult<Vec<MemoryEntry>> {
            Ok(vec![])
        }

        async fn update_relevance(&self, id: MemoryId, score: f64) -> MemoryResult<()> {
            let mut guard = self.entries.lock().expect("lock");
            guard
                .get_mut(&id.to_string())
                .ok_or_else(|| MemoryError::NotFound(id.to_string()))?
                .relevance_score = score;
            Ok(())
        }

        async fn delete_memory(&self, id: MemoryId) -> MemoryResult<()> {
            self.entries.lock().expect("lock").remove(&id.to_string());
            Ok(())
        }

        async fn count_entries(&self, agent_id: &AgentId) -> MemoryResult<usize> {
            let guard = self.entries.lock().expect("lock");
            let count = guard.values().filter(|e| e.agent_id == *agent_id).count();
            Ok(count)
        }

        async fn delete_by_age(
            &self,
            _agent_id: &AgentId,
            _max_age: chrono::Duration,
        ) -> MemoryResult<usize> {
            Ok(0)
        }

        async fn create_episode(&self, episode: &Episode) -> MemoryResult<EpisodeId> {
            Ok(episode.id)
        }

        async fn get_episode(&self, _id: EpisodeId) -> MemoryResult<Episode> {
            Err(MemoryError::NotFound("episode".into()))
        }

        async fn end_episode(&self, _id: EpisodeId, _summary: &str) -> MemoryResult<()> {
            Ok(())
        }

        async fn list_episodes(
            &self,
            _agent_id: AgentId,
            _limit: usize,
        ) -> MemoryResult<Vec<Episode>> {
            Ok(vec![])
        }

        async fn forget(&self, _artifact_id: &str) -> MemoryResult<usize> {
            Ok(0)
        }
    }

    // ── Fake PaymentStore ────────────────────────────────────────────────

    #[derive(Debug, Default)]
    struct FakePaymentStore {
        costs: Mutex<Vec<CostRecord>>,
    }

    #[async_trait::async_trait]
    impl PaymentStore for FakePaymentStore {
        async fn record_cost(&self, record: CostRecord) -> Result<(), PaymentError> {
            self.costs.lock().expect("lock").push(record);
            Ok(())
        }

        async fn get_costs(&self, run_id: &str) -> Result<Vec<CostRecord>, PaymentError> {
            let guard = self.costs.lock().expect("lock");
            Ok(guard
                .iter()
                .filter(|r| r.run_id == run_id)
                .cloned()
                .collect())
        }

        async fn get_usage(
            &self,
            _agent_id: &str,
            since: chrono::DateTime<Utc>,
            until: chrono::DateTime<Utc>,
        ) -> Result<UsageSummary, PaymentError> {
            Ok(UsageSummary {
                total_runs: 0,
                total_tokens: 0,
                estimated_usd: 0.0,
                period_start: since,
                period_end: until,
            })
        }

        async fn create_intent(&self, _intent: PaymentIntent) -> Result<(), PaymentError> {
            Ok(())
        }

        async fn get_intent(&self, id: Uuid) -> Result<PaymentIntent, PaymentError> {
            Err(PaymentError::IntentNotFound { id })
        }

        async fn update_intent_status(
            &self,
            _id: Uuid,
            _status: PaymentStatus,
        ) -> Result<(), PaymentError> {
            Ok(())
        }

        async fn create_receipt(&self, _receipt: PaymentReceipt) -> Result<(), PaymentError> {
            Ok(())
        }
    }

    // ── Fake ConversationStore ───────────────────────────────────────────

    #[derive(Debug, Default)]
    struct FakeConversationStore;

    #[async_trait::async_trait]
    impl ConversationStore for FakeConversationStore {
        async fn create(&self, _conversation: Conversation) -> ConversationResult<ConversationId> {
            Ok(ConversationId::new())
        }

        async fn get(&self, id: ConversationId) -> ConversationResult<Conversation> {
            Err(ConversationError::NotFound(id.to_string()))
        }

        async fn list(
            &self,
            _agent_id: AgentId,
            _limit: usize,
            _offset: usize,
        ) -> ConversationResult<Vec<ConversationSummary>> {
            Ok(vec![])
        }

        async fn delete(&self, _id: ConversationId) -> ConversationResult<()> {
            Ok(())
        }

        async fn add_message(
            &self,
            _conversation_id: ConversationId,
            _message: Message,
        ) -> ConversationResult<Uuid> {
            Ok(Uuid::now_v7())
        }

        async fn get_messages(
            &self,
            _conversation_id: ConversationId,
            _limit: usize,
            _offset: usize,
        ) -> ConversationResult<Vec<Message>> {
            Ok(vec![])
        }

        async fn get_recent_messages(
            &self,
            _conversation_id: ConversationId,
            _limit: usize,
        ) -> ConversationResult<Vec<Message>> {
            Ok(vec![])
        }

        async fn update_title(
            &self,
            _id: ConversationId,
            _title: String,
        ) -> ConversationResult<()> {
            Ok(())
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

    fn build_service_with_all() -> AppService {
        let run_store: Arc<dyn RunStore> = Arc::new(FakeRunStore::default());
        let event_store: Arc<dyn EventStore> = Arc::new(FakeEventStore::default());
        let bus = EventBus::new(64);
        let recorder = EventRecorder::new(event_store, bus.clone());
        let memory_store: Arc<dyn MemoryStore + Send + Sync> = Arc::new(FakeMemoryStore::default());
        let payment_store: Arc<dyn PaymentStore + Send + Sync> =
            Arc::new(FakePaymentStore::default());
        let conv_store: Arc<dyn ConversationStore + Send + Sync> = Arc::new(FakeConversationStore);

        AppService::builder()
            .with_config(Config::default())
            .with_executor(Arc::new(FakeExecutor))
            .with_run_store(run_store)
            .with_event_bus(bus)
            .with_event_recorder(recorder)
            .with_memory_store(memory_store)
            .with_payment_store(payment_store)
            .with_conversation_store(conv_store)
            .build()
            .expect("build service with all")
    }

    fn make_spec(name: &str) -> AgentSpec {
        AgentSpec::new(AgentId::new(), name, "anthropic/claude-opus-4-6")
    }

    fn make_memory_entry(agent_id: AgentId, content: &str) -> MemoryEntry {
        MemoryEntry {
            id: MemoryId::new(),
            agent_id,
            episode_id: None,
            memory_type: MemoryType::Semantic,
            content: content.to_owned(),
            embedding: None,
            metadata: serde_json::Value::Null,
            provenance: None,
            created_at: Utc::now(),
            accessed_at: Utc::now(),
            access_count: 0,
            relevance_score: 1.0,
            confidence: 1.0,
            classification: polkagent_memory::classification::Classification::Internal,
        }
    }

    // ── Original tests (all must still pass) ────────────────────────────

    #[test]
    fn builder_fails_without_config() {
        let result = AppService::builder().build();
        assert!(matches!(result, Err(ServiceError::NotInitialized { .. })));
    }

    #[test]
    fn builder_fails_without_event_recorder() {
        let result = AppService::builder()
            .with_config(Config::default())
            .with_run_store(Arc::new(FakeRunStore::default()))
            .build();
        assert!(matches!(result, Err(ServiceError::NotInitialized { .. })));
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
        assert!(matches!(result, Err(ServiceError::AgentNotFound { .. })));
    }

    /// When `max_concurrent_runs = 1` and a run is already in the `"running"`
    /// state, a second `start_run` must be rejected with
    /// `ServiceError::ConcurrentRunLimitReached`.
    #[tokio::test]
    async fn start_run_rejects_when_concurrent_limit_reached() {
        // Build a service with a shared run store we can manipulate directly.
        let run_store = Arc::new(FakeRunStore::default());
        let run_store_arc: Arc<dyn RunStore> = Arc::clone(&run_store) as _;
        let event_store: Arc<dyn EventStore> = Arc::new(FakeEventStore::default());
        let bus = EventBus::new(64);
        let recorder = EventRecorder::new(event_store, bus.clone());

        let mut config = Config::default();
        config.execution.max_concurrent_runs = 1;

        let service = AppService::builder()
            .with_config(config)
            .with_run_store(run_store_arc)
            .with_event_bus(bus)
            .with_event_recorder(recorder)
            .build()
            .expect("build");

        // Register an agent.
        let spec = make_spec("limit-test");
        let agent_id = service.create_agent(spec).expect("create_agent");

        // Manually insert a run in "running" state directly into the store
        // (bypassing the service) to simulate an already-active run.
        let occupied_run_id = RunId::new();
        run_store
            .create(
                occupied_run_id,
                &agent_id.to_string(),
                RunStatus::new("running"),
            )
            .await
            .expect("seed running run");

        // Attempting to start another run must now fail.
        let result = service.start_run(agent_id, "second run").await;
        assert!(
            matches!(
                result,
                Err(ServiceError::ConcurrentRunLimitReached {
                    active: 1,
                    limit: 1,
                })
            ),
            "expected ConcurrentRunLimitReached, got: {result:?}"
        );
    }

    /// When `max_concurrent_runs = 0` (unlimited), runs should be accepted
    /// regardless of how many are in `"running"` state.
    #[tokio::test]
    async fn start_run_unlimited_when_max_concurrent_runs_is_zero() {
        let run_store = Arc::new(FakeRunStore::default());
        let run_store_arc: Arc<dyn RunStore> = Arc::clone(&run_store) as _;
        let event_store: Arc<dyn EventStore> = Arc::new(FakeEventStore::default());
        let bus = EventBus::new(64);
        let recorder = EventRecorder::new(event_store, bus.clone());

        let mut config = Config::default();
        config.execution.max_concurrent_runs = 0; // unlimited

        let service = AppService::builder()
            .with_config(config)
            .with_run_store(run_store_arc)
            .with_event_bus(bus)
            .with_event_recorder(recorder)
            .build()
            .expect("build");

        let spec = make_spec("unlimited-test");
        let agent_id = service.create_agent(spec).expect("create_agent");

        // Seed several "running" runs.
        for _ in 0..5 {
            run_store
                .create(
                    RunId::new(),
                    &agent_id.to_string(),
                    RunStatus::new("running"),
                )
                .await
                .expect("seed running run");
        }

        // Starting another run must still succeed (limit=0 means unlimited).
        let result = service.start_run(agent_id, "overflow check").await;
        assert!(
            result.is_ok(),
            "expected Ok when max_concurrent_runs=0, got: {result:?}"
        );
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
        assert!(matches!(result, Err(ServiceError::NotInitialized { .. })));
    }

    #[tokio::test]
    async fn deny_effect_without_store_fails() {
        let service = build_service();
        let result = service.deny_effect(EffectId::new(), "test reason").await;
        assert!(matches!(result, Err(ServiceError::NotInitialized { .. })));
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

    // ── New tests ────────────────────────────────────────────────────────

    #[test]
    fn builder_with_all_services_configured() {
        let service = build_service_with_all();
        assert!(service.memory_store().is_some());
        assert!(service.payment_store().is_some());
        assert!(service.conversation_store().is_some());
        // tool_registry and skill_runner not set in build_service_with_all
        assert!(service.tool_registry().is_none());
        assert!(service.skill_runner().is_none());
    }

    #[test]
    fn builder_with_tool_registry() {
        let run_store: Arc<dyn RunStore> = Arc::new(FakeRunStore::default());
        let event_store: Arc<dyn EventStore> = Arc::new(FakeEventStore::default());
        let bus = EventBus::new(64);
        let recorder = EventRecorder::new(event_store, bus.clone());
        let tool_reg = Arc::new(polkagent_tool::ToolRegistry::new());

        let service = AppService::builder()
            .with_config(Config::default())
            .with_run_store(run_store)
            .with_event_bus(bus)
            .with_event_recorder(recorder)
            .with_tool_registry(tool_reg)
            .build()
            .expect("build");

        assert!(service.tool_registry().is_some());
    }

    #[test]
    fn builder_with_skill_runner() {
        let run_store: Arc<dyn RunStore> = Arc::new(FakeRunStore::default());
        let event_store: Arc<dyn EventStore> = Arc::new(FakeEventStore::default());
        let bus = EventBus::new(64);
        let recorder = EventRecorder::new(event_store, bus.clone());
        let runner = Arc::new(polkagent_skill::SkillRunner::new());

        let service = AppService::builder()
            .with_config(Config::default())
            .with_run_store(run_store)
            .with_event_bus(bus)
            .with_event_recorder(recorder)
            .with_skill_runner(runner)
            .build()
            .expect("build");

        assert!(service.skill_runner().is_some());
    }

    #[tokio::test]
    async fn start_run_publishes_events_to_bus() {
        let service = build_service();
        let mut rx = service.subscribe_events();

        let spec = make_spec("event-test-agent");
        let agent_id = service.create_agent(spec).expect("create_agent");

        let _run_id = service
            .start_run(agent_id, "event test prompt")
            .await
            .expect("start_run");

        // RunManager emits RunCreated then RunQueued via the EventRecorder.
        let ev1 = rx.recv().await.expect("first event");
        assert!(
            matches!(ev1.kind, EventKind::RunCreated),
            "expected RunCreated, got {:?}",
            ev1.kind
        );

        let ev2 = rx.recv().await.expect("second event");
        assert!(
            matches!(ev2.kind, EventKind::RunQueued),
            "expected RunQueued, got {:?}",
            ev2.kind
        );
    }

    #[tokio::test]
    async fn start_run_events_carry_correct_run_id() {
        let service = build_service();
        let mut rx = service.subscribe_events();

        let spec = make_spec("correlation-test");
        let agent_id = service.create_agent(spec).expect("create_agent");
        let run_id = service
            .start_run(agent_id, "test")
            .await
            .expect("start_run");

        let ev1 = rx.recv().await.expect("ev1");
        let ev2 = rx.recv().await.expect("ev2");

        assert_eq!(ev1.run_id, run_id);
        assert_eq!(ev2.run_id, run_id);
    }

    #[tokio::test]
    async fn approve_effect_channel_is_functional() {
        let service = build_service();
        let mut rx = service.subscribe_approvals();

        // Without an effect store the channel exists but is empty.
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn memory_store_absent_returns_not_initialized() {
        let service = build_service();
        let agent_id = AgentId::new();
        let entry = make_memory_entry(agent_id, "test content");

        let result = service.store_memory(&agent_id, entry).await;
        assert!(matches!(result, Err(ServiceError::NotInitialized { .. })));
    }

    #[tokio::test]
    async fn search_memory_absent_returns_not_initialized() {
        let service = build_service();
        let agent_id = AgentId::new();
        let result = service.search_memory(&agent_id, "query", 10).await;
        assert!(matches!(result, Err(ServiceError::NotInitialized { .. })));
    }

    #[tokio::test]
    async fn store_and_search_memory_delegates_to_store() {
        let service = build_service_with_all();
        let agent_id = AgentId::new();
        let entry = make_memory_entry(agent_id, "the user prefers dark mode");

        let id = service
            .store_memory(&agent_id, entry)
            .await
            .expect("store_memory");
        assert_ne!(id.to_string(), "");

        let results = service
            .search_memory(&agent_id, "dark mode", 10)
            .await
            .expect("search_memory");
        assert!(!results.is_empty());
        assert!(results[0].content.contains("dark mode"));
    }

    #[tokio::test]
    async fn record_cost_absent_returns_not_initialized() {
        let service = build_service();
        let run_id = RunId::new();
        let amount = polkagent_payment::Amount::new(100, polkagent_payment::AssetId::Native, 10);
        let result = service.record_cost(&run_id, &amount).await;
        assert!(matches!(result, Err(ServiceError::NotInitialized { .. })));
    }

    #[tokio::test]
    async fn get_usage_absent_returns_not_initialized() {
        let service = build_service();
        let agent_id = AgentId::new();
        let result = service.get_usage(&agent_id).await;
        assert!(matches!(result, Err(ServiceError::NotInitialized { .. })));
    }

    #[tokio::test]
    async fn record_cost_delegates_to_payment_store() {
        let service = build_service_with_all();
        let run_id = RunId::new();
        let amount = polkagent_payment::Amount::new(500, polkagent_payment::AssetId::Native, 10);
        let result = service.record_cost(&run_id, &amount).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn get_usage_delegates_to_payment_store() {
        let service = build_service_with_all();
        let agent_id = AgentId::new();
        let summary = service.get_usage(&agent_id).await.expect("get_usage");
        assert_eq!(summary.total_runs, 0);
    }

    #[test]
    fn debug_format_includes_new_fields() {
        let service = build_service_with_all();
        let debug = format!("{service:?}");
        assert!(debug.contains("has_memory_store"));
        assert!(debug.contains("has_payment_store"));
        assert!(debug.contains("has_conversation_store"));
        assert!(debug.contains("has_orchestrator"));
    }

    #[test]
    fn subscribe_approvals_returns_receiver() {
        let service = build_service();
        let _rx = service.subscribe_approvals();
        // No panic = success.
    }

    #[tokio::test]
    async fn multiple_agents_multiple_runs_are_independent() {
        let service = build_service();

        let spec_a = make_spec("agent-a");
        let spec_b = make_spec("agent-b");
        let agent_a = service.create_agent(spec_a).expect("create a");
        let agent_b = service.create_agent(spec_b).expect("create b");

        let run_a1 = service.start_run(agent_a, "a-1").await.expect("a1");
        let run_a2 = service.start_run(agent_a, "a-2").await.expect("a2");
        let only_b_run = service.start_run(agent_b, "b-1").await.expect("b1");

        let runs_a = service.list_runs(agent_a).await.expect("list a");
        let runs_b = service.list_runs(agent_b).await.expect("list b");

        assert_eq!(runs_a.len(), 2);
        assert_eq!(runs_b.len(), 1);

        // All runs are in Queued state.
        for run_id in [run_a1, run_a2, only_b_run] {
            let state = service.get_run_status(run_id).await.expect("status");
            assert_eq!(state, RunState::Queued);
        }
    }

    #[test]
    fn conversation_store_accessor_returns_some_when_set() {
        let service = build_service_with_all();
        assert!(service.conversation_store().is_some());
    }

    #[test]
    fn memory_store_accessor_returns_none_when_not_set() {
        let service = build_service();
        assert!(service.memory_store().is_none());
    }

    #[test]
    fn payment_store_accessor_returns_none_when_not_set() {
        let service = build_service();
        assert!(service.payment_store().is_none());
    }

    #[test]
    fn orchestrator_present_when_executor_configured() {
        let service = build_service(); // has FakeExecutor
        let debug = format!("{service:?}");
        assert!(debug.contains("has_orchestrator: true"));
    }

    #[test]
    fn orchestrator_absent_without_executor() {
        let run_store: Arc<dyn RunStore> = Arc::new(FakeRunStore::default());
        let event_store: Arc<dyn EventStore> = Arc::new(FakeEventStore::default());
        let bus = EventBus::new(64);
        let recorder = EventRecorder::new(event_store, bus.clone());

        let service = AppService::builder()
            .with_config(Config::default())
            .with_run_store(run_store)
            .with_event_bus(bus)
            .with_event_recorder(recorder)
            // No executor set
            .build()
            .expect("build service without executor");

        assert!(service.executor().is_none());
        let debug = format!("{service:?}");
        assert!(debug.contains("has_orchestrator: false"));
    }

    #[tokio::test]
    async fn start_run_without_executor_stays_queued() {
        let run_store: Arc<dyn RunStore> = Arc::new(FakeRunStore::default());
        let event_store: Arc<dyn EventStore> = Arc::new(FakeEventStore::default());
        let bus = EventBus::new(64);
        let recorder = EventRecorder::new(event_store, bus.clone());

        let service = AppService::builder()
            .with_config(Config::default())
            .with_run_store(run_store)
            .with_event_bus(bus)
            .with_event_recorder(recorder)
            .build()
            .expect("build");

        let spec = make_spec("no-exec-agent");
        let agent_id = service.create_agent(spec).expect("create_agent");
        let run_id = service
            .start_run(agent_id, "test")
            .await
            .expect("start_run");

        let state = service.get_run_status(run_id).await.expect("state");
        assert_eq!(state, RunState::Queued);
    }

    // ── Fake Signer and ChainClient for explain pipeline tests ─────────

    use polkagent_chain_trait::{
        BlockRef, ChainError, DecodedCall, FinalityObservation,
        MetadataDigest as ChainMetadataDigest, PinnedMetadata, SimulationResult, TxHash,
    };
    use polkagent_signer_trait::{
        AccountRef, CanonicalSignRequest, SignedPayload, SignerCapabilities, SignerError,
    };

    /// A mock [`Signer`] for app-level integration tests.
    #[derive(Debug)]
    struct FakeSigner {
        /// If set, `sign` returns this error.
        sign_error: Option<&'static str>,
    }

    impl FakeSigner {
        fn new() -> Self {
            Self { sign_error: None }
        }

        fn rejecting() -> Self {
            Self {
                sign_error: Some("user rejected"),
            }
        }
    }

    #[async_trait::async_trait]
    impl polkagent_signer_trait::Signer for FakeSigner {
        async fn describe(&self) -> Result<SignerCapabilities, SignerError> {
            Ok(SignerCapabilities {
                accounts: vec![AccountRef::from_bytes([0u8; 32])],
                chain_profiles: vec![polkagent_signer_trait::ChainProfileId::new("test-chain")],
                hardware_backed: false,
                display_name: "FakeSigner".into(),
                can_sign: true,
            })
        }

        async fn sign(&self, request: CanonicalSignRequest) -> Result<SignedPayload, SignerError> {
            if let Some(_msg) = &self.sign_error {
                return Err(SignerError::UserRejected);
            }
            Ok(SignedPayload {
                signed_extrinsic: [request.payload.as_slice(), &[0xFF, 0xFE]].concat(),
                public_key: vec![2u8; 32],
                signature: vec![3u8; 64],
            })
        }

        async fn health(&self) -> Result<(), SignerError> {
            Ok(())
        }
    }

    /// A mock [`ChainClient`] for app-level integration tests.
    #[derive(Debug)]
    struct FakeChainClient;

    #[async_trait::async_trait]
    impl polkagent_chain_trait::ChainClient for FakeChainClient {
        async fn fetch_metadata(
            &self,
            _chain_profile: polkagent_chain_trait::ChainProfileId,
        ) -> Result<PinnedMetadata, ChainError> {
            Ok(PinnedMetadata {
                chain_profile: polkagent_chain_trait::ChainProfileId::new("test-chain"),
                spec_version: 1_000_000,
                metadata_digest: ChainMetadataDigest("abcdef0123456789".into()),
                metadata_bytes: vec![0xDE, 0xAD, 0xBE, 0xEF],
                block_ref: BlockRef {
                    number: 42,
                    hash: "0xblockhash".into(),
                },
                fetched_at: chrono::Utc::now(),
            })
        }

        async fn simulate(
            &self,
            _signed_extrinsic: &[u8],
            _block_ref: &BlockRef,
            _metadata: &PinnedMetadata,
        ) -> Result<SimulationResult, ChainError> {
            Ok(SimulationResult {
                success: true,
                fee_estimate: Some(1_000_000),
                error_message: None,
                storage_changes_preview: vec![],
                block_ref: BlockRef {
                    number: 50,
                    hash: "0xsim".into(),
                },
            })
        }

        async fn submit_extrinsic(
            &self,
            _signed_extrinsic: &[u8],
            _chain_profile: polkagent_chain_trait::ChainProfileId,
        ) -> Result<TxHash, ChainError> {
            Ok(TxHash::new("0xtxhash"))
        }

        async fn watch_finality(
            &self,
            _tx_hash: TxHash,
            _chain_profile: polkagent_chain_trait::ChainProfileId,
            _timeout_ms: u64,
        ) -> Result<FinalityObservation, ChainError> {
            Ok(FinalityObservation::Finalized {
                block_ref: BlockRef {
                    number: 100,
                    hash: "0xfinalized".into(),
                },
                tx_index: 0,
            })
        }

        async fn decode_call(
            &self,
            _call_bytes: &[u8],
            _metadata: &PinnedMetadata,
        ) -> Result<DecodedCall, ChainError> {
            Ok(DecodedCall {
                pallet: "Balances".into(),
                call_name: "transfer_keep_alive".into(),
                arguments_json: r#"{"dest":"5GrwvaEF","value":1000}"#.into(),
                metadata_digest: ChainMetadataDigest("abcdef0123456789".into()),
            })
        }

        async fn query_storage(
            &self,
            _storage_key: &[u8],
            _block_ref: Option<&BlockRef>,
            _chain_profile: polkagent_chain_trait::ChainProfileId,
        ) -> Result<Option<Vec<u8>>, ChainError> {
            Ok(None)
        }

        async fn dry_run_call(
            &self,
            _extrinsic: &[u8],
        ) -> Result<polkagent_chain_trait::DryRunResult, ChainError> {
            Err(ChainError::Unsupported {
                operation: "dry_run_call".into(),
            })
        }

        async fn xcm_query_acceptable_payment_assets(
            &self,
            _version: u8,
        ) -> Result<Vec<String>, ChainError> {
            Err(ChainError::Unsupported {
                operation: "xcm_query_acceptable_payment_assets".into(),
            })
        }

        async fn xcm_query_delivery_fee(
            &self,
            _dest: &polkagent_chain_trait::GenesisHash,
            _message: &[u8],
        ) -> Result<u128, ChainError> {
            Err(ChainError::Unsupported {
                operation: "xcm_query_delivery_fee".into(),
            })
        }

        async fn is_trusted_teleporter(
            &self,
            _dest: &polkagent_chain_trait::ChainProfileId,
            _asset: &str,
        ) -> Result<bool, ChainError> {
            Err(ChainError::Unsupported {
                operation: "is_trusted_teleporter".into(),
            })
        }

        async fn is_reserve_transfer_supported(
            &self,
            _dest: &polkagent_chain_trait::ChainProfileId,
            _asset: &str,
        ) -> Result<bool, ChainError> {
            Err(ChainError::Unsupported {
                operation: "is_reserve_transfer_supported".into(),
            })
        }

        async fn health(&self) -> Result<(), ChainError> {
            Ok(())
        }
    }

    fn build_service_with_signer_and_chain() -> AppService {
        let run_store: Arc<dyn RunStore> = Arc::new(FakeRunStore::default());
        let event_store: Arc<dyn EventStore> = Arc::new(FakeEventStore::default());
        let bus = EventBus::new(64);
        let recorder = EventRecorder::new(event_store, bus.clone());

        AppService::builder()
            .with_config(Config::default())
            .with_run_store(run_store)
            .with_event_bus(bus)
            .with_event_recorder(recorder)
            .with_signer(Arc::new(FakeSigner::new()))
            .with_chain_client(Arc::new(FakeChainClient))
            .build()
            .expect("build service with signer and chain_client")
    }

    // ── Explain-Before-Sign pipeline tests ─────────────────────────────

    /// Test 1: Builder accepts signer and `chain_client`, accessors return Some.
    #[test]
    fn builder_accepts_signer_and_chain_client() {
        let service = build_service_with_signer_and_chain();
        assert!(service.signer().is_some());
        assert!(service.chain_client().is_some());
    }

    /// Test 2: Signer and `chain_client` are None by default.
    #[test]
    fn signer_and_chain_client_none_by_default() {
        let service = build_service();
        assert!(service.signer().is_none());
        assert!(service.chain_client().is_none());
    }

    /// Test 3: `explain_and_sign` errors with `NotInitialized` when signer is
    /// not configured.
    #[tokio::test]
    async fn explain_and_sign_errors_without_signer() {
        let run_store: Arc<dyn RunStore> = Arc::new(FakeRunStore::default());
        let event_store: Arc<dyn EventStore> = Arc::new(FakeEventStore::default());
        let bus = EventBus::new(64);
        let recorder = EventRecorder::new(event_store, bus.clone());

        // Only chain_client, no signer.
        let service = AppService::builder()
            .with_config(Config::default())
            .with_run_store(run_store)
            .with_event_bus(bus)
            .with_event_recorder(recorder)
            .with_chain_client(Arc::new(FakeChainClient))
            .build()
            .expect("build");

        let request = crate::explain::ExplainRequest {
            call_bytes: vec![0x05, 0x00, 0x01, 0x02],
            chain_profile: polkagent_chain_trait::ChainProfileId::new("test-chain"),
            run_id: RunId::new(),
            agent_id: AgentId::new(),
        };

        let result = service.explain_and_sign(request).await;
        assert!(matches!(
            result,
            Err(ServiceError::NotInitialized { ref component }) if component == "signer"
        ));
    }

    /// Test 4: `explain_and_sign` errors with `NotInitialized` when `chain_client`
    /// is not configured.
    #[tokio::test]
    async fn explain_and_sign_errors_without_chain_client() {
        let run_store: Arc<dyn RunStore> = Arc::new(FakeRunStore::default());
        let event_store: Arc<dyn EventStore> = Arc::new(FakeEventStore::default());
        let bus = EventBus::new(64);
        let recorder = EventRecorder::new(event_store, bus.clone());

        // Only signer, no chain_client.
        let service = AppService::builder()
            .with_config(Config::default())
            .with_run_store(run_store)
            .with_event_bus(bus)
            .with_event_recorder(recorder)
            .with_signer(Arc::new(FakeSigner::new()))
            .build()
            .expect("build");

        let request = crate::explain::ExplainRequest {
            call_bytes: vec![0x05, 0x00, 0x01, 0x02],
            chain_profile: polkagent_chain_trait::ChainProfileId::new("test-chain"),
            run_id: RunId::new(),
            agent_id: AgentId::new(),
        };

        let result = service.explain_and_sign(request).await;
        assert!(matches!(
            result,
            Err(ServiceError::NotInitialized { ref component }) if component == "chain_client"
        ));
    }

    /// Test 5: `explain_and_sign` succeeds when both signer and `chain_client`
    /// are configured, delegating to the full pipeline.
    #[tokio::test]
    async fn explain_and_sign_full_pipeline_succeeds() {
        let service = build_service_with_signer_and_chain();

        let request = crate::explain::ExplainRequest {
            call_bytes: vec![0x05, 0x00, 0x01, 0x02, 0x03],
            chain_profile: polkagent_chain_trait::ChainProfileId::new("test-chain"),
            run_id: RunId::new(),
            agent_id: AgentId::new(),
        };

        let result = service
            .explain_and_sign(request)
            .await
            .expect("explain_and_sign should succeed");

        assert_eq!(result.tx_hash.0, "0xtxhash");
        assert!(matches!(
            result.finality,
            FinalityObservation::Finalized { .. }
        ));
    }

    /// Test 6: `explain_and_sign` propagates signer rejection as
    /// `ExplainPipeline` error.
    #[tokio::test]
    async fn explain_and_sign_signer_rejection_propagates() {
        let run_store: Arc<dyn RunStore> = Arc::new(FakeRunStore::default());
        let event_store: Arc<dyn EventStore> = Arc::new(FakeEventStore::default());
        let bus = EventBus::new(64);
        let recorder = EventRecorder::new(event_store, bus.clone());

        let service = AppService::builder()
            .with_config(Config::default())
            .with_run_store(run_store)
            .with_event_bus(bus)
            .with_event_recorder(recorder)
            .with_signer(Arc::new(FakeSigner::rejecting()))
            .with_chain_client(Arc::new(FakeChainClient))
            .build()
            .expect("build");

        let request = crate::explain::ExplainRequest {
            call_bytes: vec![0x05, 0x00],
            chain_profile: polkagent_chain_trait::ChainProfileId::new("test-chain"),
            run_id: RunId::new(),
            agent_id: AgentId::new(),
        };

        let result = service.explain_and_sign(request).await;
        assert!(matches!(result, Err(ServiceError::ExplainPipeline { .. })));
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("rejected"));
    }

    /// Test 7: Debug format includes signer and `chain_client` fields.
    #[test]
    fn debug_format_includes_signer_and_chain_client() {
        let service = build_service_with_signer_and_chain();
        let debug = format!("{service:?}");
        assert!(debug.contains("has_signer: true"));
        assert!(debug.contains("has_chain_client: true"));

        // Verify defaults show false
        let basic = build_service();
        let debug_basic = format!("{basic:?}");
        assert!(debug_basic.contains("has_signer: false"));
        assert!(debug_basic.contains("has_chain_client: false"));
    }

    // -----------------------------------------------------------------------
    // Config watcher integration tests
    // -----------------------------------------------------------------------

    /// Write a minimal valid TOML config to a file path.
    fn write_valid_config(path: &std::path::Path, log_level: &str) {
        let content = format!(
            "[meta]\napi_version = \"polkagent.dev/v1alpha1\"\nschema_version = 1\n\n[log]\nlevel = \"{log_level}\"\n",
        );
        std::fs::write(path, content).expect("write config");
    }

    /// Test 8: Config watcher starts without error.
    #[tokio::test]
    async fn config_watcher_starts_without_error() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let config_path = tmp.path().join("polkagent.toml");
        write_valid_config(&config_path, "info");

        let service = build_service();
        let result = service.start_config_watcher(&config_path, Duration::from_millis(50));
        assert!(result.is_ok(), "watcher should start without error");

        service.stop_config_watcher();
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    /// Test 9: Initial config is available via `atomic_config` / `config()`.
    #[test]
    fn initial_config_is_available() {
        let service = build_service();
        let cfg = service.config();
        assert_eq!(cfg.meta.schema_version, 1);

        let ac = service.atomic_config();
        let cfg2 = ac.get();
        assert_eq!(cfg2.meta.schema_version, cfg.meta.schema_version);
    }

    /// Test 10: Config swap is atomic -- readers see a consistent snapshot.
    #[test]
    fn config_swap_is_atomic_readers_see_consistent_state() {
        let service = build_service();

        let before = service.config();
        assert_eq!(before.log.level, "info");

        let mut new_config = Config::default();
        new_config.log.level = "debug".to_owned();
        let _old = service.atomic_config.swap(new_config);

        assert_eq!(before.log.level, "info");

        let after = service.config();
        assert_eq!(after.log.level, "debug");
    }

    /// Test 11: Invalid config is rejected and old config is preserved.
    #[tokio::test]
    async fn invalid_config_rejected_old_config_preserved() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let config_path = tmp.path().join("polkagent.toml");
        write_valid_config(&config_path, "info");

        let service = build_service();
        service
            .start_config_watcher(&config_path, Duration::from_millis(30))
            .expect("start watcher");

        tokio::time::sleep(Duration::from_millis(100)).await;

        std::fs::write(&config_path, b"this is not valid toml [[[").expect("write");

        tokio::time::sleep(Duration::from_millis(200)).await;

        let cfg = service.config();
        assert_eq!(cfg.log.level, "info", "invalid config must not be applied");

        service.stop_config_watcher();
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    /// Test 12: Watcher can be stopped gracefully.
    #[tokio::test]
    async fn watcher_stops_gracefully() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let config_path = tmp.path().join("polkagent.toml");
        write_valid_config(&config_path, "info");

        let service = build_service();
        service
            .start_config_watcher(&config_path, Duration::from_millis(30))
            .expect("start watcher");

        service.stop_config_watcher();
        tokio::time::sleep(Duration::from_millis(150)).await;

        let guard = service.watcher_shutdown.lock().expect("lock");
        assert!(
            guard.is_none(),
            "shutdown sender should be consumed after stop"
        );
    }

    /// Test 13: Starting a second watcher while one is running returns an error.
    #[tokio::test]
    async fn double_start_watcher_returns_error() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let config_path = tmp.path().join("polkagent.toml");
        write_valid_config(&config_path, "info");

        let service = build_service();
        service
            .start_config_watcher(&config_path, Duration::from_millis(50))
            .expect("first start");

        let result = service.start_config_watcher(&config_path, Duration::from_millis(50));
        assert!(
            matches!(result, Err(ServiceError::ConfigReload { .. })),
            "double start should return ConfigReload error, got: {result:?}"
        );

        service.stop_config_watcher();
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    /// Test 14: Valid config change is picked up by the watcher.
    #[tokio::test]
    async fn valid_config_change_is_applied_by_watcher() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let config_path = tmp.path().join("polkagent.toml");
        write_valid_config(&config_path, "info");

        let service = build_service();
        service
            .start_config_watcher(&config_path, Duration::from_millis(30))
            .expect("start watcher");

        tokio::time::sleep(Duration::from_millis(100)).await;

        write_valid_config(&config_path, "debug");

        tokio::time::sleep(Duration::from_millis(300)).await;

        let cfg = service.config();
        assert_eq!(
            cfg.log.level, "debug",
            "watcher should apply the new config"
        );

        service.stop_config_watcher();
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    /// Test 15: Config deletion preserves old config.
    #[tokio::test]
    async fn config_deletion_preserves_old_config() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let config_path = tmp.path().join("polkagent.toml");
        write_valid_config(&config_path, "warn");

        let service = build_service();

        let mut initial = Config::default();
        initial.log.level = "warn".to_owned();
        service.atomic_config.swap(initial);

        service
            .start_config_watcher(&config_path, Duration::from_millis(30))
            .expect("start watcher");

        tokio::time::sleep(Duration::from_millis(100)).await;

        std::fs::remove_file(&config_path).expect("remove");

        tokio::time::sleep(Duration::from_millis(200)).await;

        let cfg = service.config();
        assert_eq!(
            cfg.log.level, "warn",
            "config deletion should preserve the current config"
        );

        service.stop_config_watcher();
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    /// Test 16: `stop_config_watcher` is idempotent.
    #[test]
    fn stop_config_watcher_is_idempotent() {
        let service = build_service();
        service.stop_config_watcher();
        service.stop_config_watcher();
    }

    /// Test 17: `atomic_config` returns a clonable Arc handle.
    #[test]
    fn atomic_config_handle_is_shareable_across_threads() {
        let service = build_service();
        let handle = service.atomic_config();
        let handle2 = Arc::clone(&handle);

        let cfg1 = handle.get();
        let cfg2 = handle2.get();
        assert_eq!(cfg1.meta.schema_version, cfg2.meta.schema_version);

        let mut new_cfg = Config::default();
        new_cfg.log.level = "trace".to_owned();
        handle.swap(new_cfg);

        let cfg3 = handle2.get();
        assert_eq!(cfg3.log.level, "trace");
    }
}
