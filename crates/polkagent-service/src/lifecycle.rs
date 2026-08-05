//! Application lifecycle management.
//!
//! The [`startup`] and [`shutdown`] functions handle the full initialization
//! and teardown sequence for the Polkagent platform. They orchestrate config
//! loading, telemetry initialization, database setup, provider registration,
//! event bus creation, and service construction.
//!
//! # Startup sequence
//!
//! 1. Load and validate configuration.
//! 2. Initialize telemetry/logging.
//! 3. Open / migrate the database (future: `SQLite` / Postgres).
//! 4. Register configured providers.
//! 5. Initialize event bus and recorder.
//! 6. Construct [`AppService`].
//!
//! # Shutdown sequence
//!
//! 1. Cancel all running runs.
//! 2. Flush pending events.
//! 3. Close database connections.

use std::sync::Arc;
use std::time::Duration;

use polkagent_config::Config;
use polkagent_event::{EventBus, EventRecorder};
use polkagent_executor_trait::ModelExecutor;
use polkagent_run::TimeoutConfig;
use polkagent_store_trait::event::EventStore;
use polkagent_store_trait::{EffectStore, RunStatus, RunStore};
use tracing::{info, warn};

use crate::app::AppService;
use crate::error::ServiceError;
use crate::provider::ProviderRegistry;

// ---------------------------------------------------------------------------
// StartupContext
// ---------------------------------------------------------------------------

/// External dependencies injected into the startup process.
///
/// In production, these are created from the configuration (e.g. opening a
/// `SQLite` database). In tests, they are fake in-memory implementations.
pub struct StartupContext {
    /// The run store implementation.
    pub run_store: Arc<dyn RunStore>,
    /// The event store implementation.
    pub event_store: Arc<dyn EventStore>,
    /// Optional effect store. When provided, effects are persisted to this
    /// store. When `None`, the builder falls back to `NoopEffectStore` which
    /// logs warnings and discards effects.
    pub effect_store: Option<Arc<dyn EffectStore>>,
    /// Optional default model executor.
    pub executor: Option<Arc<dyn ModelExecutor>>,
    /// Pre-built provider registry (providers already registered).
    pub provider_registry: Option<ProviderRegistry>,
}

// ---------------------------------------------------------------------------
// startup
// ---------------------------------------------------------------------------

/// Perform full application initialization and return a ready-to-use
/// [`AppService`].
///
/// This function takes an already-loaded [`Config`] and a [`StartupContext`]
/// containing the external dependencies (stores, executor). In production
/// the caller opens the database and constructs the stores before calling
/// this function.
///
/// # Steps
///
/// 1. Validate the configuration.
/// 2. Create the event bus.
/// 3. Create the event recorder.
/// 4. Build the [`AppService`] via its builder.
///
/// # Errors
///
/// Returns [`ServiceError`] if configuration is invalid or a required
/// component fails to initialize.
pub fn startup(config: Config, context: StartupContext) -> Result<AppService, ServiceError> {
    info!(
        schema_version = config.meta.schema_version,
        "starting polkagent service"
    );

    // Step 1: Validate config.
    polkagent_config::validate::validate(&config).map_err(|errors| {
        let messages: Vec<String> = errors
            .iter()
            .map(std::string::ToString::to_string)
            .collect();
        ServiceError::Config {
            message: format!("configuration validation failed: {}", messages.join("; ")),
        }
    })?;
    info!("configuration validated");

    // Step 2: Create event bus.
    let event_bus = EventBus::with_default_capacity();
    info!(
        capacity = polkagent_event::bus::DEFAULT_CAPACITY,
        "event bus created"
    );

    // Step 3: Create event recorder.
    let recorder = EventRecorder::new(context.event_store, event_bus.clone());

    // Step 4: Build service.
    let mut builder = AppService::builder()
        .with_config(config)
        .with_run_store(context.run_store)
        .with_event_bus(event_bus)
        .with_event_recorder(recorder);

    if let Some(effect_store) = context.effect_store {
        builder = builder.with_effect_store(effect_store);
    }

    if let Some(executor) = context.executor {
        builder = builder.with_executor(executor);
    }

    if let Some(registry) = context.provider_registry {
        builder = builder.with_provider_registry(registry);
    }

    let service = builder.build()?;

    // Step 5: Start the timeout enforcer background task.
    //
    // Uses the `default_timeout_secs` from the execution config as the global
    // max duration, and sweeps every 30 seconds. This requires a Tokio runtime
    // to be active (which is the case for all production entry points).
    let timeout_secs = service.config().execution.default_timeout_secs;
    let timeout_config = TimeoutConfig::with_global_max(Duration::from_secs(timeout_secs));
    service.start_timeout_enforcer(timeout_config, Duration::from_secs(30));

    info!("polkagent service started successfully");

    Ok(service)
}

// ---------------------------------------------------------------------------
// recover_stuck_runs (startup reaper)
// ---------------------------------------------------------------------------

/// Non-terminal run states that should not survive a process restart.
///
/// Any run found in one of these states during startup is assumed to have been
/// abandoned when the previous process exited and is transitioned to `failed`.
const STUCK_STATES: &[&str] = &[
    "running",
    "queued",
    "completing",
    "awaiting_approval",
    "waiting_effect",
];

/// Reason retained when crash recovery terminates an abandoned run.
const RESTART_RECOVERY_REASON: &str = "recovered after restart";

/// Scan for runs stuck in non-terminal states and transition them to `failed`.
///
/// This should be called once during startup, **after** the database is
/// available but before the service begins accepting new work. It acts as a
/// crash-recovery mechanism: any run that was in-flight when the previous
/// process exited is marked as failed with a recovery reason.
///
/// Returns the total number of recovered runs.
///
/// # Errors
///
/// Returns [`ServiceError`] if a store query or update fails. Partial
/// recovery is possible: runs that were already transitioned before the
/// error occurred remain in their new state.
pub async fn recover_stuck_runs(run_store: &dyn RunStore) -> Result<usize, ServiceError> {
    let mut recovered = 0usize;

    for &state_str in STUCK_STATES {
        let status = RunStatus::new(state_str);
        // Fetch up to 10 000 runs per state; in practice there should be very
        // few (if any) after a clean shutdown.
        let stuck = run_store
            .list_by_state(status, 10_000, 0)
            .await
            .map_err(|e| ServiceError::Store {
                message: format!("failed to list runs in state '{state_str}': {e}"),
            })?;

        for run in &stuck {
            let failed_status = RunStatus::new(format!("failed:{RESTART_RECOVERY_REASON}"));
            if let Err(e) = run_store.update_state(run.id, failed_status).await {
                warn!(
                    run_id = %run.id,
                    previous_state = state_str,
                    %e,
                    "failed to recover stuck run"
                );
            } else {
                info!(
                    run_id = %run.id,
                    previous_state = state_str,
                    "recovered stuck run: process restarted"
                );
                recovered += 1;
            }
        }
    }

    if recovered > 0 {
        info!(recovered, "startup reaper: recovered stuck runs");
    } else {
        info!("startup reaper: no stuck runs found");
    }

    Ok(recovered)
}

// ---------------------------------------------------------------------------
// shutdown
// ---------------------------------------------------------------------------

/// Perform graceful shutdown of the application service.
///
/// # Steps
///
/// 1. Log the shutdown initiation.
/// 2. Drop the service (which drops all internal Arc references).
///
/// In a full implementation, this would:
/// - Cancel all running runs with a "service shutdown" reason.
/// - Flush any buffered events to the event store.
/// - Close database connections.
///
/// The current implementation logs the shutdown and drops the service,
/// which triggers `Arc` reference count decrements.
pub fn shutdown(service: AppService) {
    info!("shutting down polkagent service");

    // Drop the service, releasing all Arc-backed resources.
    // In a production implementation:
    // 1. service.cancel_all_running_runs().await
    // 2. service.flush_events().await
    // 3. service.close_database().await
    drop(service);

    info!("polkagent service shut down");
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use polkagent_core::RunId;
    use polkagent_store_trait::{
        event::{EventFilter, EventStoreError, StoredEvent},
        RunStatus, RunSummary, StoreError,
    };
    use std::collections::{HashMap, HashSet};
    use std::sync::Mutex;

    // ── Fake stores ─────────────────────────────────────────────────────

    #[derive(Debug, Default)]
    struct FakeRunStore {
        runs: Mutex<HashMap<String, RunSummary>>,
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
            guard.insert(
                run_id.to_string(),
                RunSummary {
                    id: run_id,
                    agent_id: agent_id.to_owned(),
                    status,
                    created_at: chrono::Utc::now(),
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
                .ok_or(StoreError::NotFound {
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
                .ok_or(StoreError::NotFound {
                    resource_type: "Run",
                    id: run_id.to_string(),
                })?
                .status = new_status;
            Ok(())
        }

        async fn list_by_agent(
            &self,
            _agent_id: &str,
            _limit: u32,
            _offset: u32,
        ) -> Result<Vec<RunSummary>, StoreError> {
            Ok(vec![])
        }

        async fn list_by_state(
            &self,
            _status: RunStatus,
            _limit: u32,
            _offset: u32,
        ) -> Result<Vec<RunSummary>, StoreError> {
            Ok(vec![])
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

    const TERMINAL_TYPES: &[&str] = &[
        "run_completed",
        "run_failed",
        "run_cancelled",
        "run_timed_out",
    ];

    #[derive(Debug, Default)]
    struct FakeEventStore {
        durable: Mutex<Vec<StoredEvent>>,
        sequences: Mutex<HashMap<String, u64>>,
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
            _cursor: u64,
            _limit: usize,
        ) -> Result<Vec<StoredEvent>, EventStoreError> {
            Ok(vec![])
        }

        async fn read_run_events(
            &self,
            _run_id: RunId,
        ) -> Result<Vec<StoredEvent>, EventStoreError> {
            Ok(vec![])
        }

        async fn query(&self, _filter: EventFilter) -> Result<Vec<StoredEvent>, EventStoreError> {
            Ok(vec![])
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

    fn fake_context() -> StartupContext {
        StartupContext {
            run_store: Arc::new(FakeRunStore::default()),
            event_store: Arc::new(FakeEventStore::default()),
            effect_store: None,
            executor: None,
            provider_registry: None,
        }
    }

    // ── Tests ───────────────────────────────────────────────────────────

    #[tokio::test]
    async fn startup_with_default_config_succeeds() {
        let config = Config::default();
        let service = startup(config, fake_context()).expect("startup");
        assert_eq!(
            service.config().meta.schema_version,
            polkagent_config::CURRENT_SCHEMA_VERSION
        );
    }

    #[tokio::test]
    async fn startup_and_shutdown_do_not_panic() {
        let config = Config::default();
        let service = startup(config, fake_context()).expect("startup");
        shutdown(service);
    }

    #[tokio::test]
    async fn startup_produces_working_service() {
        let config = Config::default();
        let service = startup(config, fake_context()).expect("startup");

        // Create an agent and start a run.
        let spec = polkagent_core::AgentSpec::new(
            polkagent_core::AgentId::new(),
            "lifecycle-test",
            "fake/model",
        );
        let agent_id = service.create_agent(spec).expect("create_agent");

        let run_id = service
            .start_run(agent_id, "test prompt")
            .await
            .expect("start_run");

        let state = service
            .get_run_status(run_id)
            .await
            .expect("get_run_status");
        assert_eq!(state, polkagent_core::RunState::Queued);

        shutdown(service);
    }
}
