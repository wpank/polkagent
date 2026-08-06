//! APR-08 recovery evidence for the possible-external-I/O boundary.
//!
//! The fixture crosses only production ports: a real file-backed `SQLite`
//! coordinator persists an approval, claim, checkpoint, and attempt start;
//! then a newly composed `AppService` leases and recovers that checkpoint.

#![allow(
    clippy::expect_used,
    clippy::too_many_lines,
    reason = "the crash fixture keeps one exact durable lineage visible from seed through two process replacements"
)]

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{Duration as ChronoDuration, Utc};
use polkagent_config::{Config, SecurityConfig};
use polkagent_conversation::{Conversation, ConversationStore};
use polkagent_core::{
    AgentId, AgentSpec, ApprovalId, ConversationId, DataClassification, EffectAttemptId, EffectId,
    PrincipalId, RunId, StepId, TurnId, WorkerId,
};
use polkagent_event::{EventBus, EventRecorder};
use polkagent_executor_trait::{
    ContentBlock, InferenceMessage, MessageRole, ModelExecutor, TokenUsage, ToolCall,
};
use polkagent_grant::{
    grant::{GrantResolver, ResolverConfig},
    policy::{Effect, PolicyRule, PolicySet},
};
use polkagent_run::ApprovalRuntimeConfig;
use polkagent_service::{AppService, ServiceError};
use polkagent_store_sqlite::{migrations, SqlitePool, SqliteRunStore};
use polkagent_store_trait::approval::{
    compute_approval_subject_digest, compute_execution_checkpoint_digest,
    compute_policy_snapshot_digest, compute_tool_spec_digest, ApprovalCoordinatorStore,
    ApprovalDecision, ApprovalPrincipalType, ApprovalRequestMetadata, ApprovalScope,
    ApprovalStatus, ApprovalSubject, CheckpointEffect, CheckpointEffectStatus, CheckpointStatus,
    ClaimApprovedEffect, ExecutionCheckpoint, ExecutionCheckpointStore, PauseForApproval,
    ResolveApproval, APPROVAL_SUBJECT_SCHEMA_VERSION, EXECUTION_CHECKPOINT_SCHEMA_VERSION,
};
use polkagent_store_trait::{EffectStore, RunStatus, RunStore, StoreRetryClass};
use polkagent_tool::{ToolContext, ToolError, ToolHandler, ToolRegistry, ToolResult, ToolSpec};

const TENANT: &str = "apr08-possible-io-tenant";
const WORKSPACE: &str = "apr08-possible-io-workspace";
const TOOL_NAME: &str = "test.possible_io";
const REQUIRED_ACTION: &str = "test.write";
const REQUIRED_RESOURCE: &str = "tool/test.possible_io";
const TOOL_CALL_ID: &str = "call-apr08-possible-io";
const RECONCILIATION_REASON: &str =
    "an attempt may have started but no durable outcome exists; automatic retry is forbidden";
const LEASE: Duration = Duration::from_millis(25);

struct CountingTool {
    invocations: Arc<AtomicUsize>,
}

#[async_trait]
impl ToolHandler for CountingTool {
    fn spec(&self) -> ToolSpec {
        tool_spec()
    }

    async fn execute(
        &self,
        _input: serde_json::Value,
        _context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        self.invocations.fetch_add(1, Ordering::SeqCst);
        Ok(ToolResult {
            output: serde_json::json!({"unexpected": "recovery invoked the handler"}),
            classification: DataClassification::Public,
            artifacts: Vec::new(),
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
struct DurableLineage {
    approval_id: String,
    approval_effect_id: String,
    approval_run_id: String,
    approval_turn_id: String,
    approval_conversation_id: String,
    approval_agent_id: String,
    approval_status: String,
    effect_id: String,
    effect_run_id: String,
    effect_turn_id: String,
    effect_step_id: String,
    effect_approval_id: Option<String>,
    effect_state: String,
    run_id: String,
    run_agent_id: String,
    run_conversation_id: Option<String>,
    run_state: String,
    run_state_version: u64,
    checkpoint_run_id: String,
    checkpoint_turn_id: String,
    checkpoint_conversation_id: String,
    checkpoint_agent_id: String,
    checkpoint_version: u64,
    checkpoint_status: String,
    attempt_id: Option<String>,
    attempt_count: u64,
    outcome_count: u64,
}

fn tool_spec() -> ToolSpec {
    ToolSpec {
        name: TOOL_NAME.to_owned(),
        description: "APR-08 possible-I/O counting tool".to_owned(),
        input_schema: serde_json::json!({"type": "object"}),
        required_grant: Some(REQUIRED_ACTION.to_owned()),
        output_classification: DataClassification::Public,
    }
}

fn approval_policy() -> PolicySet {
    PolicySet::new(vec![PolicyRule {
        id: "apr08-possible-io".to_owned(),
        effect: Effect::RequireApproval,
        action_patterns: vec![REQUIRED_ACTION.to_owned()],
        resource_patterns: vec![REQUIRED_RESOURCE.to_owned()],
        conditions: HashMap::default(),
        abac_condition: None,
    }])
}

fn open_migrated(path: &Path) -> SqlitePool {
    let pool = SqlitePool::open(path).expect("open APR-08 recovery database");
    {
        let writer = pool.writer();
        migrations::migrate(&writer).expect("migrate APR-08 recovery database");
    }
    pool
}

fn build_service(
    pool: &SqlitePool,
    agent: &AgentSpec,
    principal_id: PrincipalId,
    service_principal_id: PrincipalId,
    workdir: &Path,
    invocations: Arc<AtomicUsize>,
) -> AppService {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(CountingTool { invocations }));
    let event_bus = EventBus::new(32);
    let service = AppService::builder()
        .with_config(Config::default())
        .with_executor(polkagent_executor_fake::FakeExecutor::new() as Arc<dyn ModelExecutor>)
        .with_run_store(Arc::new(pool.clone()))
        .with_effect_store(Arc::new(pool.clone()))
        .with_grant_resolver(GrantResolver::new(
            approval_policy(),
            ResolverConfig::default(),
        ))
        .with_event_bus(event_bus.clone())
        .with_event_recorder(EventRecorder::new(Arc::new(pool.clone()), event_bus))
        .with_tool_registry(Arc::new(registry))
        .with_conversation_store(Arc::new(pool.clone()))
        .with_approval_runtime(
            Arc::new(pool.clone()),
            Arc::new(pool.clone()),
            ApprovalRuntimeConfig {
                tenant_id: TENANT.to_owned(),
                workspace_id: WORKSPACE.to_owned(),
                authorized_principal_id: principal_id,
                service_principal_id,
                working_directory: workdir.to_path_buf(),
                security_config: SecurityConfig::default(),
                approval_timeout: Duration::from_secs(300),
                recovery_lease: LEASE,
                poll_interval: Duration::from_millis(5),
            },
        )
        .build()
        .expect("build APR-08 recovery service");
    service
        .create_agent(agent.clone())
        .expect("register APR-08 recovery agent");
    service
}

fn durable_lineage(pool: &SqlitePool, approval_id: ApprovalId) -> DurableLineage {
    pool.writer()
        .query_row(
            "SELECT
                approval.id, approval.effect_id, approval.run_id, approval.turn_id,
                approval.conversation_id, approval.agent_id, approval.status,
                effect.id, effect.run_id, effect.turn_id, effect.step_id,
                effect.approval_id, effect.state,
                run.id, run.agent_id, run.conversation_id, run.state, run.state_version,
                checkpoint.run_id, checkpoint.turn_id, checkpoint.conversation_id,
                checkpoint.agent_id, checkpoint.version, checkpoint.status,
                (SELECT attempt.id FROM effect_attempts attempt
                 WHERE attempt.intent_id = effect.id ORDER BY attempt.attempt_number LIMIT 1),
                (SELECT COUNT(*) FROM effect_attempts attempt
                 WHERE attempt.intent_id = effect.id),
                (SELECT COUNT(*) FROM effect_outcomes outcome
                 WHERE outcome.intent_id = effect.id)
             FROM approval_requests approval
             JOIN effect_intents effect ON effect.id = approval.effect_id
             JOIN runs run ON run.id = approval.run_id
             JOIN execution_checkpoints checkpoint ON checkpoint.run_id = approval.run_id
             WHERE approval.id = ?1",
            [approval_id.to_string()],
            |row| {
                Ok(DurableLineage {
                    approval_id: row.get(0)?,
                    approval_effect_id: row.get(1)?,
                    approval_run_id: row.get(2)?,
                    approval_turn_id: row.get(3)?,
                    approval_conversation_id: row.get(4)?,
                    approval_agent_id: row.get(5)?,
                    approval_status: row.get(6)?,
                    effect_id: row.get(7)?,
                    effect_run_id: row.get(8)?,
                    effect_turn_id: row.get(9)?,
                    effect_step_id: row.get(10)?,
                    effect_approval_id: row.get(11)?,
                    effect_state: row.get(12)?,
                    run_id: row.get(13)?,
                    run_agent_id: row.get(14)?,
                    run_conversation_id: row.get(15)?,
                    run_state: row.get(16)?,
                    run_state_version: row.get(17)?,
                    checkpoint_run_id: row.get(18)?,
                    checkpoint_turn_id: row.get(19)?,
                    checkpoint_conversation_id: row.get(20)?,
                    checkpoint_agent_id: row.get(21)?,
                    checkpoint_version: row.get(22)?,
                    checkpoint_status: row.get(23)?,
                    attempt_id: row.get(24)?,
                    attempt_count: row.get(25)?,
                    outcome_count: row.get(26)?,
                })
            },
        )
        .expect("load exact APR-08 durable lineage")
}

fn assert_manual_reconciliation(error: ServiceError, run_id: RunId, effect_id: EffectId) {
    assert!(matches!(
        error,
        ServiceError::ManualReconciliation {
            run_id: actual_run,
            effect_id: actual_effect,
            reason,
        } if actual_run == run_id
            && actual_effect == effect_id
            && reason == RECONCILIATION_REASON
    ));
}

#[tokio::test]
async fn executing_approved_effect_without_outcome_fails_closed_across_restarts() {
    let temporary = tempfile::tempdir().expect("APR-08 temporary directory");
    let workdir = temporary.path().canonicalize().expect("canonical workdir");
    let database_path = workdir.join("possible-io-recovery.sqlite");
    let pool = open_migrated(&database_path);
    let store = SqliteRunStore::new(pool.clone());
    let agent_row = store
        .create_agent("apr08-possible-io-agent", None, "{}")
        .expect("persist APR-08 recovery agent");
    let agent_id: AgentId = agent_row.id.parse().expect("typed agent ID");
    let mut agent = AgentSpec::new(agent_id, "apr08-possible-io-agent", "fixture/model");
    agent.tools = vec![TOOL_NAME.to_owned()];
    let conversation_id = ConversationId::new();
    ConversationStore::create(&pool, Conversation::new(conversation_id, agent_id))
        .await
        .expect("persist APR-08 recovery conversation");
    let run_id = RunId::new();
    let agent_id_text = agent_id.to_string();
    let conversation_id_text = conversation_id.to_string();
    RunStore::create_correlated(
        &pool,
        run_id,
        &agent_id_text,
        Some(&conversation_id_text),
        RunStatus::new("running"),
    )
    .await
    .expect("persist running APR-08 recovery run");
    let turn_id = TurnId::new();
    let step_id = StepId::new();
    let started_at = Utc::now().to_rfc3339();
    RunStore::insert_turn(
        &pool,
        turn_id,
        run_id,
        1,
        "assistant",
        &started_at,
        None,
        0,
        0,
    )
    .await
    .expect("persist APR-08 recovery turn");
    RunStore::insert_step(&pool, step_id, turn_id, 1, "tool_call", &started_at)
        .await
        .expect("persist APR-08 recovery step");

    let principal_id = PrincipalId::new();
    let service_principal_id = PrincipalId::new();
    let invocations = Arc::new(AtomicUsize::new(0));
    let original_service = build_service(
        &pool,
        &agent,
        principal_id,
        service_principal_id,
        &workdir,
        Arc::clone(&invocations),
    );
    let effect_id = EffectId::new();
    let approval_id = ApprovalId::new();
    let attempt_id = EffectAttemptId::new();
    let worker_id = WorkerId::new();
    let deadline = Utc::now() + ChronoDuration::minutes(5);
    let arguments = serde_json::json!({"path": "approved.txt", "value": 7});
    let spec = tool_spec();
    let mut subject = ApprovalSubject {
        schema_version: APPROVAL_SUBJECT_SCHEMA_VERSION,
        conversation_id,
        turn_id,
        step_id,
        run_id,
        agent_id,
        effect_id,
        tool_call_id: TOOL_CALL_ID.to_owned(),
        tool_name: TOOL_NAME.to_owned(),
        validated_arguments: arguments.clone(),
        required_action: REQUIRED_ACTION.to_owned(),
        required_resource: REQUIRED_RESOURCE.to_owned(),
        working_directory: Some(workdir.to_string_lossy().into_owned()),
        security_scope: serde_json::json!({
            "tenant_id": TENANT,
            "workspace_id": WORKSPACE,
            "working_directory": workdir.to_string_lossy(),
            "security_config": SecurityConfig::default(),
        }),
        tool_spec_digest: compute_tool_spec_digest(&spec).expect("tool-spec digest"),
        policy_snapshot_digest: compute_policy_snapshot_digest(&approval_policy())
            .expect("policy digest"),
        subject_digest: String::new(),
    };
    subject.subject_digest =
        compute_approval_subject_digest(&subject).expect("approval-subject digest");
    let tool_call = ToolCall {
        tool_call_id: TOOL_CALL_ID.to_owned(),
        tool_name: TOOL_NAME.to_owned(),
        arguments_json: arguments.to_string(),
    };
    let messages = vec![InferenceMessage {
        role: MessageRole::Assistant,
        content: vec![ContentBlock::ToolUse {
            tool_call_id: TOOL_CALL_ID.to_owned(),
            tool_name: TOOL_NAME.to_owned(),
            arguments_json: arguments.to_string(),
        }],
    }];
    let mut checkpoint = ExecutionCheckpoint {
        schema_version: EXECUTION_CHECKPOINT_SCHEMA_VERSION,
        run_id,
        turn_id,
        conversation_id,
        agent_id,
        model_id: "fixture/model".to_owned(),
        executor_id: "fixture".to_owned(),
        messages: serde_json::to_value(messages).expect("checkpoint messages"),
        tool_calls: serde_json::json!({
            "approval_id": approval_id,
            "calls": [tool_call],
        }),
        next_model_turn: 2,
        next_step_sequence: 2,
        next_effect_sequence: 2,
        accumulated_usage: serde_json::to_value(TokenUsage::default()).expect("checkpoint usage"),
        accumulated_cost: None,
        deadline_at: Some(deadline),
        retry_class: StoreRetryClass::CheckBeforeRetry,
        effects: vec![CheckpointEffect {
            effect_id,
            status: CheckpointEffectStatus::AwaitingApproval,
        }],
        version: 1,
        classification: DataClassification::Private,
        retention_expires_at: None,
        integrity_digest: String::new(),
    };
    checkpoint.integrity_digest =
        compute_execution_checkpoint_digest(&checkpoint).expect("checkpoint digest");
    pool.pause_for_approval(PauseForApproval {
        approval_id,
        expected_run_state_version: 0,
        subject: subject.clone(),
        effect_payload: serde_json::json!({
            "kind": "tool_call",
            "tool_call_id": TOOL_CALL_ID,
            "tool_name": TOOL_NAME,
            "arguments": arguments,
        }),
        effect_idempotency_key: format!("apr08-possible-io-{effect_id}"),
        retry_class: StoreRetryClass::CheckBeforeRetry,
        checkpoint,
        metadata: ApprovalRequestMetadata {
            title: "Approve possible-I/O fixture".to_owned(),
            description: "Allow this exact deterministic fixture once".to_owned(),
            reason: "APR-08 possible-I/O recovery evidence".to_owned(),
            tenant_id: TENANT.to_owned(),
            workspace_id: WORKSPACE.to_owned(),
            authorized_principal_id: principal_id,
        },
        deadline_at: deadline,
    })
    .await
    .expect("pause exact APR-08 effect");
    let scope = ApprovalScope {
        tenant_id: TENANT.to_owned(),
        workspace_id: WORKSPACE.to_owned(),
        conversation_id: Some(conversation_id),
        principal_id,
    };
    let approval = pool
        .resolve_approval(ResolveApproval {
            approval_id,
            expected_run_state_version: 1,
            effect_id,
            run_id,
            turn_id,
            conversation_id,
            subject_digest: subject.subject_digest.clone(),
            scope: scope.clone(),
            principal_id,
            principal_type: ApprovalPrincipalType::Human,
            surface: "apr08-recovery-fixture".to_owned(),
            decision: ApprovalDecision::AllowOnce,
            rationale: Some("reviewed exact fixture".to_owned()),
            conditions: Vec::new(),
        })
        .await
        .expect("approve exact APR-08 effect")
        .approval;
    assert_eq!(approval.status, ApprovalStatus::Approved);
    let checkpoint = pool
        .get_checkpoint(run_id)
        .await
        .expect("load decided checkpoint");
    assert_eq!(checkpoint.status, CheckpointStatus::Resumable);
    let claim = pool
        .claim_approved_effect(ClaimApprovedEffect {
            approval_id,
            effect_id,
            run_id,
            subject_digest: subject.subject_digest,
            expected_checkpoint_version: checkpoint.checkpoint.version,
            worker_id,
            lease_duration: LEASE,
        })
        .await
        .expect("claim exact approved APR-08 effect");
    pool.record_attempt_start(
        attempt_id,
        effect_id,
        claim.worker_id,
        serde_json::json!({"boundary": "possible_io"}),
    )
    .await
    .expect("persist possible-I/O attempt boundary");

    let before_crash = durable_lineage(&pool, approval_id);
    assert_eq!(before_crash.approval_status, "approved");
    assert_eq!(before_crash.effect_state, "executing");
    assert_eq!(
        before_crash.run_state,
        format!("waiting_effect:{effect_id}")
    );
    assert_eq!(before_crash.attempt_id, Some(attempt_id.to_string()));
    assert_eq!(before_crash.attempt_count, 1);
    assert_eq!(before_crash.outcome_count, 0);
    assert_eq!(invocations.load(Ordering::SeqCst), 0);

    drop(original_service);
    drop(store);
    drop(pool);
    tokio::time::sleep(LEASE.saturating_mul(3)).await;

    let first_pool = open_migrated(&database_path);
    let first_restart = build_service(
        &first_pool,
        &agent,
        principal_id,
        service_principal_id,
        &workdir,
        Arc::clone(&invocations),
    );
    let first_error = first_restart
        .recover_approval_checkpoints()
        .await
        .expect_err("possible-I/O recovery must fail closed");
    assert_manual_reconciliation(first_error, run_id, effect_id);
    assert_eq!(invocations.load(Ordering::SeqCst), 0);
    assert_eq!(durable_lineage(&first_pool, approval_id), before_crash);
    drop(first_restart);
    drop(first_pool);
    tokio::time::sleep(LEASE.saturating_mul(3)).await;

    let second_pool = open_migrated(&database_path);
    let second_restart = build_service(
        &second_pool,
        &agent,
        principal_id,
        service_principal_id,
        &workdir,
        Arc::clone(&invocations),
    );
    let second_error = second_restart
        .recover_approval_checkpoints()
        .await
        .expect_err("second recovery must preserve manual reconciliation");
    assert_manual_reconciliation(second_error, run_id, effect_id);
    assert_eq!(invocations.load(Ordering::SeqCst), 0);
    assert_eq!(durable_lineage(&second_pool, approval_id), before_crash);
}
