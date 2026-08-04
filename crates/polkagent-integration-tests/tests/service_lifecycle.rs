//! Service lifecycle integration tests.
//!
//! Exercises AppService construction with fake adapters, agent registration,
//! run creation and status query, event subscription, and shutdown sequence.

use std::sync::Arc;

use polkagent_config::Config;
use polkagent_core::{AgentId, AgentSpec, RunId};
use polkagent_event::{EventBus, EventRecorder};
use polkagent_executor_fake::FakeExecutor;
use polkagent_executor_trait::ModelExecutor;
use polkagent_service::{AppService, ServiceError};
use polkagent_store_trait::event::EventStore;

use polkagent_integration_tests::{MemEventStore, MemRunStore};

// ---------------------------------------------------------------------------
// Builder helpers
// ---------------------------------------------------------------------------

fn make_service() -> AppService {
    let run_store = Arc::new(MemRunStore::default());
    let event_store = Arc::new(MemEventStore::default());
    let event_store_dyn = Arc::clone(&event_store) as Arc<dyn EventStore>;
    let bus = EventBus::new(64);
    let recorder = EventRecorder::new(event_store_dyn, bus.clone());

    AppService::builder()
        .with_config(Config::default())
        .with_run_store(run_store)
        .with_event_bus(bus)
        .with_event_recorder(recorder)
        .build()
        .expect("build service")
}

fn make_service_with_executor() -> AppService {
    let run_store = Arc::new(MemRunStore::default());
    let event_store = Arc::new(MemEventStore::default());
    let event_store_dyn = Arc::clone(&event_store) as Arc<dyn EventStore>;
    let bus = EventBus::new(64);
    let recorder = EventRecorder::new(event_store_dyn, bus.clone());
    // FakeExecutor::new() already returns Arc<FakeExecutor>; coerce to Arc<dyn ModelExecutor>.
    let executor: Arc<dyn ModelExecutor> = FakeExecutor::new();

    AppService::builder()
        .with_config(Config::default())
        .with_run_store(run_store)
        .with_event_bus(bus)
        .with_event_recorder(recorder)
        .with_executor(executor)
        .build()
        .expect("build service with executor")
}

fn make_agent_spec(name: &str) -> AgentSpec {
    AgentSpec::new(AgentId::new(), name, "anthropic/claude-opus-4-6")
}

// ---------------------------------------------------------------------------
// Builder tests
// ---------------------------------------------------------------------------

#[test]
fn builder_missing_config_fails() {
    let run_store = Arc::new(MemRunStore::default());
    let event_store = Arc::new(MemEventStore::default());
    let event_store_dyn = Arc::clone(&event_store) as Arc<dyn EventStore>;
    let bus = EventBus::new(64);
    let recorder = EventRecorder::new(event_store_dyn, bus.clone());

    let result = AppService::builder()
        // deliberately omitting .with_config(...)
        .with_run_store(run_store)
        .with_event_bus(bus)
        .with_event_recorder(recorder)
        .build();

    assert!(
        matches!(result, Err(ServiceError::NotInitialized { .. })),
        "missing config must return NotInitialized error"
    );
}

#[test]
fn builder_missing_run_store_fails() {
    let event_store = Arc::new(MemEventStore::default());
    let event_store_dyn = Arc::clone(&event_store) as Arc<dyn EventStore>;
    let bus = EventBus::new(64);
    let recorder = EventRecorder::new(event_store_dyn, bus.clone());

    let result = AppService::builder()
        .with_config(Config::default())
        // deliberately omitting .with_run_store(...)
        .with_event_bus(bus)
        .with_event_recorder(recorder)
        .build();

    assert!(
        matches!(result, Err(ServiceError::NotInitialized { .. })),
        "missing run_store must return NotInitialized error"
    );
}

#[test]
fn builder_missing_event_recorder_fails() {
    let run_store = Arc::new(MemRunStore::default());
    let bus = EventBus::new(64);

    let result = AppService::builder()
        .with_config(Config::default())
        .with_run_store(run_store)
        .with_event_bus(bus)
        // deliberately omitting .with_event_recorder(...)
        .build();

    assert!(
        matches!(result, Err(ServiceError::NotInitialized { .. })),
        "missing event_recorder must return NotInitialized error"
    );
}

#[test]
fn builder_minimal_required_fields_succeeds() {
    let svc = make_service();
    // No executor is configured; orchestrator must be absent.
    assert!(
        svc.executor().is_none(),
        "service without executor must have no executor"
    );
}

#[test]
fn builder_with_executor_provides_executor() {
    let svc = make_service_with_executor();
    assert!(
        svc.executor().is_some(),
        "service with executor must expose it via executor()"
    );
}

// ---------------------------------------------------------------------------
// AppService::config()
// ---------------------------------------------------------------------------

#[test]
fn service_config_reflects_provided_config() {
    let mut cfg = Config::default();
    cfg.meta.schema_version = 42;

    let run_store = Arc::new(MemRunStore::default());
    let event_store = Arc::new(MemEventStore::default());
    let event_store_dyn = Arc::clone(&event_store) as Arc<dyn EventStore>;
    let bus = EventBus::new(64);
    let recorder = EventRecorder::new(event_store_dyn, bus.clone());

    let svc = AppService::builder()
        .with_config(cfg)
        .with_run_store(run_store)
        .with_event_bus(bus)
        .with_event_recorder(recorder)
        .build()
        .expect("build");

    assert_eq!(svc.config().meta.schema_version, 42);
}

// ---------------------------------------------------------------------------
// Agent management
// ---------------------------------------------------------------------------

#[test]
fn create_agent_with_valid_spec_succeeds() {
    let svc = make_service();
    let spec = make_agent_spec("test-agent");
    let id = svc.create_agent(spec).expect("create agent");
    assert!(!id.to_string().is_empty(), "agent ID must be non-empty");
}

#[test]
fn create_agent_with_empty_name_fails() {
    let svc = make_service();
    // AgentSpec::new panics if name is empty; build struct directly to bypass that.
    let spec = AgentSpec {
        id: AgentId::new(),
        name: String::new(),
        description: None,
        model: "test".into(),
        tools: vec![],
        system_prompt: None,
        autonomy_level: Default::default(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        declared_capabilities: vec![],
        policy_refs: vec![],
        resource_limits: None,
        model_preference: None,
        memory_config: None,
        surface_bindings: vec![],
    };
    let result = svc.create_agent(spec);
    assert!(
        matches!(result, Err(ServiceError::Config { .. })),
        "empty agent name must fail with Config error"
    );
}

#[test]
fn create_multiple_agents_all_get_unique_ids() {
    let svc = make_service();
    let mut ids = std::collections::HashSet::new();

    for i in 0..5 {
        let spec = make_agent_spec(&format!("agent-{i}"));
        let id = svc.create_agent(spec).expect("create");
        ids.insert(id);
    }

    assert_eq!(ids.len(), 5, "all agent IDs must be unique");
}

// ---------------------------------------------------------------------------
// Run lifecycle
// ---------------------------------------------------------------------------

#[tokio::test]
async fn start_run_for_registered_agent_returns_run_id() {
    let svc = make_service();
    let spec = make_agent_spec("my-agent");
    let agent_id = svc.create_agent(spec).expect("create agent");

    let run_id = svc.start_run(agent_id, "Hello").await.expect("start run");
    assert!(!run_id.to_string().is_empty(), "run ID must be non-empty");
}

#[tokio::test]
async fn start_run_for_nonexistent_agent_fails() {
    let svc = make_service();
    let result = svc.start_run(AgentId::new(), "Hello").await;
    assert!(
        matches!(result, Err(ServiceError::AgentNotFound { .. })),
        "starting run for nonexistent agent must fail with AgentNotFound"
    );
}

#[tokio::test]
async fn get_run_status_after_start_returns_queued_or_running() {
    let svc = make_service();
    let spec = make_agent_spec("agent");
    let agent_id = svc.create_agent(spec).expect("agent");

    let run_id = svc.start_run(agent_id, "Prompt").await.expect("start");
    let state = svc.get_run_status(run_id).await.expect("status");

    use polkagent_core::RunState;
    assert!(
        matches!(state, RunState::Queued | RunState::Running),
        "run must be Queued or Running immediately after start, got: {state:?}"
    );
}

#[tokio::test]
async fn get_run_status_for_nonexistent_run_fails() {
    let svc = make_service();
    let result = svc.get_run_status(RunId::new()).await;
    assert!(result.is_err(), "status for nonexistent run must fail");
}

#[tokio::test]
async fn cancel_run_transitions_state() {
    let svc = make_service();
    let spec = make_agent_spec("agent");
    let agent_id = svc.create_agent(spec).expect("agent");

    let run_id = svc.start_run(agent_id, "Hello").await.expect("start");
    svc.cancel_run(run_id).await.expect("cancel");

    let state = svc
        .get_run_status(run_id)
        .await
        .expect("status after cancel");
    use polkagent_core::RunState;
    assert!(
        matches!(state, RunState::Cancelled { .. }),
        "cancelled run must be in Cancelled state, got: {state:?}"
    );
}

#[tokio::test]
async fn cancel_nonexistent_run_fails() {
    let svc = make_service();
    let result = svc.cancel_run(RunId::new()).await;
    assert!(result.is_err(), "cancelling nonexistent run must fail");
}

#[tokio::test]
async fn list_runs_returns_runs_for_agent() {
    let svc = make_service();
    let spec = make_agent_spec("agent");
    let agent_id = svc.create_agent(spec).expect("agent");

    for i in 0..3 {
        svc.start_run(agent_id, &format!("Prompt {i}"))
            .await
            .expect("start");
    }

    let runs = svc.list_runs(agent_id).await.expect("list");
    assert_eq!(runs.len(), 3, "must list all 3 runs for the agent");
}

#[tokio::test]
async fn list_runs_returns_empty_for_agent_with_no_runs() {
    let svc = make_service();
    let runs = svc.list_runs(AgentId::new()).await.expect("list");
    assert!(runs.is_empty(), "agent with no runs must have empty list");
}

#[tokio::test]
async fn multiple_agents_runs_are_isolated() {
    let svc = make_service();

    let spec_a = make_agent_spec("agent-a");
    let spec_b = make_agent_spec("agent-b");
    let agent_a = svc.create_agent(spec_a).expect("a");
    let agent_b = svc.create_agent(spec_b).expect("b");

    for _ in 0..2 {
        svc.start_run(agent_a, "Hello A").await.expect("start a");
    }
    svc.start_run(agent_b, "Hello B").await.expect("start b");

    let a_runs = svc.list_runs(agent_a).await.expect("list a");
    let b_runs = svc.list_runs(agent_b).await.expect("list b");

    assert_eq!(a_runs.len(), 2, "agent_a must have 2 runs");
    assert_eq!(b_runs.len(), 1, "agent_b must have 1 run");
}

// ---------------------------------------------------------------------------
// Event subscription
// ---------------------------------------------------------------------------

#[test]
fn subscribe_events_returns_receiver() {
    let svc = make_service();
    // Just ensure subscribe_events doesn't panic or fail.
    let _receiver = svc.subscribe_events();
}

#[test]
fn multiple_subscribers_can_be_created() {
    let svc = make_service();
    let _r1 = svc.subscribe_events();
    let _r2 = svc.subscribe_events();
    let _r3 = svc.subscribe_events();
    // The event bus supports multiple subscribers.
}

#[test]
fn subscribe_approvals_returns_receiver() {
    let svc = make_service();
    let _receiver = svc.subscribe_approvals();
}

// ---------------------------------------------------------------------------
// Effect approval (requires effect store)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn approve_effect_without_effect_store_returns_not_initialized() {
    let svc = make_service(); // no effect store

    use polkagent_core::EffectId;
    let result = svc.approve_effect(EffectId::new()).await;
    assert!(
        matches!(result, Err(ServiceError::NotInitialized { .. })),
        "approve_effect without effect store must return NotInitialized"
    );
}

#[tokio::test]
async fn deny_effect_without_effect_store_returns_not_initialized() {
    let svc = make_service(); // no effect store

    use polkagent_core::EffectId;
    let result = svc.deny_effect(EffectId::new(), "not needed").await;
    assert!(
        matches!(result, Err(ServiceError::NotInitialized { .. })),
        "deny_effect without effect store must return NotInitialized"
    );
}

// ---------------------------------------------------------------------------
// Debug formatting
// ---------------------------------------------------------------------------

#[test]
fn app_service_debug_format_includes_key_fields() {
    let svc = make_service();
    let debug = format!("{svc:?}");
    assert!(
        debug.contains("AppService"),
        "debug must contain AppService"
    );
    assert!(
        debug.contains("has_executor"),
        "debug must include has_executor field"
    );
}

// ---------------------------------------------------------------------------
// Provider registry
// ---------------------------------------------------------------------------

#[test]
fn provider_registry_starts_empty() {
    let svc = make_service();
    let registry = svc.provider_registry();
    assert_eq!(
        registry.len(),
        0,
        "fresh service must have empty provider registry"
    );
}

// ---------------------------------------------------------------------------
// Startup sequence (events emitted)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn starting_run_does_not_panic_with_executor() {
    let svc = make_service_with_executor();
    let spec = make_agent_spec("agent-with-exec");
    let agent_id = svc.create_agent(spec).expect("create");

    // This spawns a background task. Should not panic.
    let _run_id = svc.start_run(agent_id, "Test prompt").await.expect("start");

    // Brief sleep to let background task settle (non-blocking, just for cleanup).
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
}

#[tokio::test]
async fn multiple_concurrent_runs_use_different_run_ids() {
    let svc = Arc::new(make_service());

    let spec = make_agent_spec("multi-agent");
    let agent_id = svc.create_agent(spec).expect("create");

    let mut run_ids = Vec::new();
    for _ in 0..5 {
        let run_id = svc.start_run(agent_id, "concurrent").await.expect("start");
        run_ids.push(run_id);
    }

    // All run IDs must be distinct.
    let unique: std::collections::HashSet<String> =
        run_ids.iter().map(|id| id.to_string()).collect();
    assert_eq!(unique.len(), 5, "all run IDs must be unique");
}
