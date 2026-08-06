#![allow(
    clippy::expect_used,
    reason = "integration-test setup and assertions intentionally stop at the first violated durability invariant"
)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::Stream;
use polkagent_config::{Config, SecurityConfig};
use polkagent_conversation::{Conversation, ConversationStore};
use polkagent_core::config::DataClassification;
use polkagent_core::{AgentId, AgentSpec, ConversationId, EventKind, PrincipalId, RunId, RunState};
use polkagent_event::{EventBus, EventRecorder};
use polkagent_executor_trait::{
    ExecutorError, InferenceRequest, InferenceResponse, ModelExecutor, StreamEvent, TokenUsage,
    ToolCall,
};
use polkagent_grant::{
    grant::{GrantResolver, ResolverConfig},
    policy::{Effect, PolicyRule, PolicySet},
};
use polkagent_run::ApprovalRuntimeConfig;
use polkagent_service::{AppService, ServiceError};
use polkagent_store_sqlite::{migrations, SqlitePool};
use polkagent_store_trait::approval::{
    ApprovalCoordinatorStore, ApprovalDecision, ApprovalPage, ApprovalPrincipalType, ApprovalScope,
    ResolveApproval,
};
use polkagent_store_trait::event::EventStore;
use polkagent_store_trait::{EffectStore, RunStore};
use polkagent_tool::{ToolContext, ToolError, ToolHandler, ToolRegistry, ToolResult, ToolSpec};

struct ScriptedExecutor {
    calls: AtomicUsize,
    requests: Mutex<Vec<InferenceRequest>>,
}

impl ScriptedExecutor {
    fn new() -> Self {
        Self {
            calls: AtomicUsize::new(0),
            requests: Mutex::new(Vec::new()),
        }
    }

    fn requests(&self) -> Vec<InferenceRequest> {
        self.requests.lock().expect("request lock").clone()
    }
}

#[async_trait]
impl ModelExecutor for ScriptedExecutor {
    async fn complete(
        &self,
        request: InferenceRequest,
    ) -> Result<InferenceResponse, ExecutorError> {
        self.requests.lock().expect("request lock").push(request);
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            Ok(InferenceResponse {
                text: "checking tools".to_owned(),
                tool_calls: vec![
                    ToolCall {
                        tool_call_id: "success-call".to_owned(),
                        tool_name: "test.observe".to_owned(),
                        arguments_json: r#"{"value":7}"#.to_owned(),
                    },
                    ToolCall {
                        tool_call_id: "grant-call".to_owned(),
                        tool_name: "test.granted".to_owned(),
                        arguments_json: "{}".to_owned(),
                    },
                    ToolCall {
                        tool_call_id: "unallowlisted-call".to_owned(),
                        tool_name: "test.unallowlisted".to_owned(),
                        arguments_json: "{}".to_owned(),
                    },
                    ToolCall {
                        tool_call_id: "unknown-call".to_owned(),
                        tool_name: "test.unknown".to_owned(),
                        arguments_json: "{}".to_owned(),
                    },
                    ToolCall {
                        tool_call_id: "malformed-call".to_owned(),
                        tool_name: "test.observe".to_owned(),
                        arguments_json: "{".to_owned(),
                    },
                    ToolCall {
                        tool_call_id: "handler-error-call".to_owned(),
                        tool_name: "test.fails".to_owned(),
                        arguments_json: r#"{"reason":"expected"}"#.to_owned(),
                    },
                ],
                stop_reason: "tool_use".to_owned(),
                usage: TokenUsage::default(),
                provider_request_id: None,
            })
        } else {
            Ok(InferenceResponse {
                text: "done".to_owned(),
                tool_calls: Vec::new(),
                stop_reason: "end_turn".to_owned(),
                usage: TokenUsage::default(),
                provider_request_id: None,
            })
        }
    }

    async fn stream(
        &self,
        _request: InferenceRequest,
    ) -> Result<
        Box<dyn Stream<Item = Result<StreamEvent, ExecutorError>> + Send + Unpin>,
        ExecutorError,
    > {
        Err(ExecutorError::Internal {
            message: "streaming is not used by this test".to_owned(),
        })
    }

    async fn health(&self) -> Result<(), ExecutorError> {
        Ok(())
    }
}

struct AuditedTool {
    spec: ToolSpec,
    pool: SqlitePool,
    invocations: Arc<AtomicUsize>,
    fail: bool,
}

#[async_trait]
impl ToolHandler for AuditedTool {
    fn spec(&self) -> ToolSpec {
        self.spec.clone()
    }

    async fn execute(
        &self,
        input: serde_json::Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let durable_lineage = {
            let writer = self.pool.writer();
            writer
                .query_row(
                    "SELECT COUNT(*) \
                     FROM steps s \
                     JOIN turns t ON t.id = s.turn_id \
                     JOIN effect_intents i ON i.step_id = s.id AND i.turn_id = t.id \
                     JOIN effect_attempts a ON a.intent_id = i.id \
                     WHERE s.id = ?1 AND t.run_id = ?2 AND i.claimed_by IS NOT NULL",
                    [context.step_id.to_string(), context.run_id.to_string()],
                    |row| row.get::<_, i64>(0),
                )
                .expect("query durable tool lineage")
        };
        assert_eq!(
            durable_lineage, 1,
            "turn, step, claimed intent, and attempt must exist before handler I/O"
        );
        if self.spec.required_grant.is_some() {
            assert_eq!(context.grants.len(), 1, "approved call has one exact grant");
        }
        self.invocations.fetch_add(1, Ordering::SeqCst);
        if self.fail {
            Err(ToolError::ExecutionFailed {
                reason: "expected handler failure".to_owned(),
            })
        } else {
            Ok(ToolResult {
                output: serde_json::json!({"observed":"durable", "input":input}),
                classification: DataClassification::Public,
                artifacts: Vec::new(),
            })
        }
    }
}

struct ApprovalExecutor {
    calls: AtomicUsize,
    requests: Mutex<Vec<InferenceRequest>>,
}

impl ApprovalExecutor {
    fn new() -> Self {
        Self {
            calls: AtomicUsize::new(0),
            requests: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl ModelExecutor for ApprovalExecutor {
    async fn complete(
        &self,
        request: InferenceRequest,
    ) -> Result<InferenceResponse, ExecutorError> {
        self.requests.lock().expect("request lock").push(request);
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            Ok(InferenceResponse {
                text: "approval needed".to_owned(),
                tool_calls: vec![ToolCall {
                    tool_call_id: "approved-call".to_owned(),
                    tool_name: "test.granted".to_owned(),
                    arguments_json: r#"{"value":9}"#.to_owned(),
                }],
                stop_reason: "tool_use".to_owned(),
                usage: TokenUsage::default(),
                provider_request_id: None,
            })
        } else {
            Ok(InferenceResponse {
                text: "approved result received".to_owned(),
                tool_calls: Vec::new(),
                stop_reason: "end_turn".to_owned(),
                usage: TokenUsage::default(),
                provider_request_id: None,
            })
        }
    }

    async fn stream(
        &self,
        _request: InferenceRequest,
    ) -> Result<
        Box<dyn Stream<Item = Result<StreamEvent, ExecutorError>> + Send + Unpin>,
        ExecutorError,
    > {
        Err(ExecutorError::Internal {
            message: "streaming is not used by this test".to_owned(),
        })
    }

    async fn health(&self) -> Result<(), ExecutorError> {
        Ok(())
    }
}

fn tool_spec(name: &str, required_grant: Option<&str>) -> ToolSpec {
    ToolSpec {
        name: name.to_owned(),
        description: format!("test handler for {name}"),
        input_schema: serde_json::json!({"type":"object"}),
        required_grant: required_grant.map(str::to_owned),
        output_classification: DataClassification::Public,
    }
}

fn open_migrated(path: &std::path::Path) -> SqlitePool {
    let pool = SqlitePool::open(path).expect("open SQLite");
    {
        let writer = pool.writer();
        migrations::migrate(&writer).expect("migrate SQLite");
    }
    pool
}

fn seed_agent(pool: &SqlitePool, spec: &AgentSpec) {
    let id = spec.id.to_string();
    let name = spec.name.clone();
    let spec_json = serde_json::to_string(spec).expect("serialize agent");
    let created_at = spec.created_at.to_rfc3339();
    let updated_at = spec.updated_at.to_rfc3339();
    pool.writer()
        .execute(
            "INSERT INTO agents (id, name, state, spec_json, created_at, updated_at) \
             VALUES (?1, ?2, 'active', ?3, ?4, ?5)",
            [&id, &name, &spec_json, &created_at, &updated_at],
        )
        .expect("seed durable agent");
}

async fn wait_for_terminal(service: &AppService, run_id: RunId) -> RunState {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let state = service.get_run_status(run_id).await.expect("run status");
            if state.is_terminal() {
                return state;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("run reached terminal state")
}

#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "one vertical-slice test keeps pre-I/O, model-loop, event, and restart assertions on the same run"
)]
async fn app_service_executes_only_allowlisted_grantless_tools_with_durable_sqlite_lineage() {
    let temp = tempfile::tempdir().expect("tempdir");
    let db_path = temp.path().join("tool-execution.db");
    let pool = open_migrated(&db_path);
    let mut agent = AgentSpec::new(AgentId::new(), "tool-agent", "test/model");
    agent.tools = vec![
        "test.observe".to_owned(),
        "test.granted".to_owned(),
        "test.unknown".to_owned(),
        "test.fails".to_owned(),
    ];
    seed_agent(&pool, &agent);

    let success_calls = Arc::new(AtomicUsize::new(0));
    let grant_calls = Arc::new(AtomicUsize::new(0));
    let unallowlisted_calls = Arc::new(AtomicUsize::new(0));
    let failure_calls = Arc::new(AtomicUsize::new(0));
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(AuditedTool {
        spec: tool_spec("test.observe", None),
        pool: pool.clone(),
        invocations: Arc::clone(&success_calls),
        fail: false,
    }));
    registry.register(Box::new(AuditedTool {
        spec: tool_spec("test.granted", Some("test.grant")),
        pool: pool.clone(),
        invocations: Arc::clone(&grant_calls),
        fail: false,
    }));
    registry.register(Box::new(AuditedTool {
        spec: tool_spec("test.unallowlisted", None),
        pool: pool.clone(),
        invocations: Arc::clone(&unallowlisted_calls),
        fail: false,
    }));
    registry.register(Box::new(AuditedTool {
        spec: tool_spec("test.fails", None),
        pool: pool.clone(),
        invocations: Arc::clone(&failure_calls),
        fail: true,
    }));

    let executor = Arc::new(ScriptedExecutor::new());
    let bus = EventBus::new(128);
    let mut events = bus.subscribe();
    let event_store: Arc<dyn EventStore> = Arc::new(pool.clone());
    let recorder = EventRecorder::new(event_store, bus.clone());
    let run_store: Arc<dyn RunStore> = Arc::new(pool.clone());
    let effect_store: Arc<dyn EffectStore> = Arc::new(pool.clone());
    let service = AppService::builder()
        .with_config(Config::default())
        .with_executor(Arc::clone(&executor) as Arc<dyn ModelExecutor>)
        .with_run_store(run_store)
        .with_effect_store(effect_store)
        .with_event_bus(bus)
        .with_event_recorder(recorder)
        .with_tool_registry(Arc::new(registry))
        .build()
        .expect("build executable tool service");
    service.create_agent(agent.clone()).expect("register agent");

    let run_id = service
        .start_run(agent.id, "exercise the tool loop")
        .await
        .expect("start run");
    assert_eq!(
        wait_for_terminal(&service, run_id).await,
        RunState::Completed
    );
    assert_eq!(success_calls.load(Ordering::SeqCst), 1);
    assert_eq!(failure_calls.load(Ordering::SeqCst), 1);
    assert_eq!(grant_calls.load(Ordering::SeqCst), 0);
    assert_eq!(unallowlisted_calls.load(Ordering::SeqCst), 0);

    let requests = executor.requests();
    assert_eq!(requests.len(), 2);
    let advertised: Vec<&str> = requests[0]
        .tools
        .iter()
        .map(|definition| definition.name.as_str())
        .collect();
    assert_eq!(advertised, vec!["test.observe", "test.fails"]);
    let results = &requests[1]
        .messages
        .last()
        .expect("tool result message")
        .content;
    assert_eq!(results.len(), 6);
    let expected_success = serde_json::to_string(&ToolResult {
        output: serde_json::json!({"observed":"durable", "input":{"value":7}}),
        classification: DataClassification::Public,
        artifacts: Vec::new(),
    })
    .expect("serialize expected result");
    match &results[0] {
        polkagent_executor_trait::ContentBlock::ToolResult {
            content, is_error, ..
        } => {
            assert!(!is_error);
            assert_eq!(content, &expected_success);
        }
        other => panic!("expected exact tool result, got {other:?}"),
    }
    let expected_error_types = [
        "approval_required",
        "not_allowlisted",
        "not_registered",
        "invalid_input",
        "execution_failed",
    ];
    for (result, expected_type) in results[1..].iter().zip(expected_error_types) {
        match result {
            polkagent_executor_trait::ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                assert!(*is_error);
                let typed: serde_json::Value =
                    serde_json::from_str(content).expect("typed tool error JSON");
                assert_eq!(typed["error"]["type"], expected_type);
            }
            other => panic!("expected typed tool error, got {other:?}"),
        }
    }

    let mut correlated_tool_events = Vec::new();
    while let Ok(event) = events.try_recv() {
        if matches!(
            event.kind,
            EventKind::ToolCallStarted { .. } | EventKind::ToolCallCompleted { .. }
        ) {
            assert_eq!(event.correlation.run_id, run_id);
            assert!(event.correlation.turn_id.is_some());
            assert!(event.correlation.step_id.is_some());
            assert!(event.correlation.effect_intent_id.is_some());
            assert!(event.correlation.effect_attempt_id.is_some());
            correlated_tool_events.push(event);
        }
    }
    assert_eq!(correlated_tool_events.len(), 4);

    drop(service);
    drop(pool);
    let reopened = open_migrated(&db_path);
    let intents = EffectStore::get_by_run(&reopened, run_id)
        .await
        .expect("reload intents after restart");
    assert_eq!(intents.len(), 2);
    assert!(intents.iter().all(|intent| intent.state == "resolved"));
    let durable_counts: (i64, i64, i64) = reopened
        .writer()
        .query_row(
            "SELECT \
               (SELECT COUNT(*) FROM steps s JOIN turns t ON t.id = s.turn_id \
                WHERE t.run_id = ?1 AND s.completed_at IS NOT NULL), \
               (SELECT COUNT(*) FROM effect_attempts a JOIN effect_intents i ON i.id = a.intent_id \
                WHERE i.run_id = ?1), \
               (SELECT COUNT(*) FROM effect_outcomes o JOIN effect_intents i ON i.id = o.intent_id \
                WHERE i.run_id = ?1)",
            [run_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("reload durable tool rows after restart");
    assert_eq!(durable_counts, (2, 2, 2));
}

#[tokio::test]
async fn approval_allow_once_executes_exactly_once_and_resumes_the_model() {
    let temp = tempfile::tempdir().expect("tempdir");
    let pool = open_migrated(&temp.path().join("approval-execution.db"));
    let mut agent = AgentSpec::new(AgentId::new(), "approval-agent", "test/model");
    agent.tools = vec!["test.granted".to_owned()];
    seed_agent(&pool, &agent);
    let conversation_id = ConversationId::new();
    ConversationStore::create(&pool, Conversation::new(conversation_id, agent.id))
        .await
        .expect("seed conversation");

    let invocations = Arc::new(AtomicUsize::new(0));
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(AuditedTool {
        spec: tool_spec("test.granted", Some("test.grant")),
        pool: pool.clone(),
        invocations: Arc::clone(&invocations),
        fail: false,
    }));
    let resolver = GrantResolver::new(
        PolicySet::new(vec![PolicyRule {
            id: "approval-test".to_owned(),
            effect: Effect::RequireApproval,
            action_patterns: vec!["test.grant".to_owned()],
            resource_patterns: vec!["tool/test.granted".to_owned()],
            conditions: HashMap::default(),
            abac_condition: None,
        }]),
        ResolverConfig::default(),
    );
    let principal_id = PrincipalId::new();
    let service_principal_id = PrincipalId::new();
    let approval_config = ApprovalRuntimeConfig {
        tenant_id: "tenant-test".to_owned(),
        workspace_id: "workspace-test".to_owned(),
        authorized_principal_id: principal_id,
        service_principal_id,
        working_directory: temp.path().to_path_buf(),
        security_config: SecurityConfig::default(),
        approval_timeout: Duration::from_secs(10),
        recovery_lease: Duration::from_secs(10),
        poll_interval: Duration::from_millis(5),
    };
    let executor = Arc::new(ApprovalExecutor::new());
    let bus = EventBus::new(128);
    let service = AppService::builder()
        .with_config(Config::default())
        .with_executor(Arc::clone(&executor) as Arc<dyn ModelExecutor>)
        .with_run_store(Arc::new(pool.clone()))
        .with_effect_store(Arc::new(pool.clone()))
        .with_grant_resolver(resolver)
        .with_event_bus(bus.clone())
        .with_event_recorder(EventRecorder::new(Arc::new(pool.clone()), bus))
        .with_tool_registry(Arc::new(registry))
        .with_conversation_store(Arc::new(pool.clone()))
        .with_approval_runtime(
            Arc::new(pool.clone()),
            Arc::new(pool.clone()),
            approval_config,
        )
        .build()
        .expect("build approval-capable service");
    assert!(service.approval_executor_ready());
    service.create_agent(agent.clone()).expect("register agent");

    let run_id = RunId::new();
    let prepared = service
        .prepare_interaction_run(run_id, agent.id, conversation_id)
        .await
        .expect("prepare correlated run");
    service
        .execute_prepared_run(prepared, "request approval")
        .await
        .expect("start prepared run");

    let scope = ApprovalScope {
        tenant_id: "tenant-test".to_owned(),
        workspace_id: "workspace-test".to_owned(),
        conversation_id: Some(conversation_id),
        principal_id,
    };
    let approval = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let mut pending = pool
                .list_pending(
                    scope.clone(),
                    ApprovalPage {
                        limit: 10,
                        offset: 0,
                    },
                )
                .await
                .expect("list pending");
            if let Some(approval) = pending.pop() {
                break approval;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("approval became pending");
    assert_eq!(invocations.load(Ordering::SeqCst), 0);
    let expected_run_state_version = RunStore::state_version(&pool, run_id)
        .await
        .expect("approval run version");
    pool.resolve_approval(ResolveApproval {
        approval_id: approval.id,
        expected_run_state_version,
        effect_id: approval.subject.effect_id,
        run_id,
        turn_id: approval.subject.turn_id,
        conversation_id,
        subject_digest: approval.subject.subject_digest,
        scope,
        principal_id,
        principal_type: ApprovalPrincipalType::Human,
        surface: "integration-test".to_owned(),
        decision: ApprovalDecision::AllowOnce,
        rationale: Some("exact call reviewed".to_owned()),
        conditions: Vec::new(),
    })
    .await
    .expect("allow exact call");

    assert_eq!(
        wait_for_terminal(&service, run_id).await,
        RunState::Completed
    );
    assert_eq!(invocations.load(Ordering::SeqCst), 1);
    assert_eq!(executor.calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        EffectStore::get_by_run(&pool, run_id)
            .await
            .expect("approval effect")
            .len(),
        1
    );
}

#[test]
fn executable_tool_registry_requires_a_real_effect_store() {
    let temp = tempfile::tempdir().expect("tempdir");
    let pool = open_migrated(&temp.path().join("composition.db"));
    let bus = EventBus::new(16);
    let recorder = EventRecorder::new(Arc::new(pool.clone()), bus.clone());
    let result = AppService::builder()
        .with_config(Config::default())
        .with_executor(Arc::new(ScriptedExecutor::new()))
        .with_run_store(Arc::new(pool))
        .with_event_bus(bus)
        .with_event_recorder(recorder)
        .with_tool_registry(Arc::new(ToolRegistry::new()))
        .build();
    assert!(matches!(
        result,
        Err(ServiceError::NotInitialized { ref component })
            if component.contains("effect_store")
    ));
}
