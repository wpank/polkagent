//! APR-08 recovery evidence around the external-I/O attempt boundary.
//!
//! The fixture crosses only production ports: a real file-backed `SQLite`
//! coordinator persists an approval and checkpoint, then optionally a claim
//! and attempt. A newly composed `AppService` proves four adjacent outcomes:
//! an approved resumable checkpoint is claimed and executed exactly once, a
//! pre-I/O claim is safely reclaimed exactly once, and a recorded attempt
//! without an outcome stops for manual reconciliation and is never retried.
//! The same fail-closed result holds if external database corruption deletes
//! the outcome from a transactionally valid resolved-effect/outcome pair.

#![allow(
    clippy::expect_used,
    clippy::too_many_lines,
    reason = "the adjacent crash/corruption fixtures keep exact durable lineage visible from seed through two process replacements"
)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{Duration as ChronoDuration, Utc};
use polkagent_config::{Config, SecurityConfig};
use polkagent_conversation::{Conversation, ConversationStore};
use polkagent_core::{
    turn::TokenUsage, AgentId, AgentSpec, ApprovalId, ConversationId, DataClassification,
    EffectAttemptId, EffectId, EffectOutcomeId, PrincipalId, RunId, StepId, TurnId, WorkerId,
};
use polkagent_effect::{idempotency::IdempotencyKey, types::EffectKind};
use polkagent_event::{EventBus, EventRecorder};
use polkagent_executor_trait::{
    ContentBlock, InferenceMessage, MessageRole, ModelExecutor, ToolCall,
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
use polkagent_store_trait::{EffectStore, RunStatus, RunStore, StoreRetryClass, StoredOutcome};
use polkagent_tool::{ToolContext, ToolError, ToolHandler, ToolRegistry, ToolResult, ToolSpec};

const TENANT: &str = "apr08-possible-io-tenant";
const WORKSPACE: &str = "apr08-possible-io-workspace";
const TOOL_NAME: &str = "test.possible_io";
const REQUIRED_ACTION: &str = "test.write";
const REQUIRED_RESOURCE: &str = "tool/test.possible_io";
const TOOL_CALL_ID: &str = "call-apr08-possible-io";
const RECONCILIATION_REASON: &str =
    "an attempt may have started but no durable outcome exists; automatic retry is forbidden";
const SEEDED_CRASH_LEASE: Duration = Duration::from_millis(25);
const RECOVERY_LEASE: Duration = Duration::from_secs(1);

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
            output: serde_json::json!({"executed": true}),
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
    step_completed: bool,
    attempt_id: Option<String>,
    attempt_intent_id: Option<String>,
    attempt_worker_id: Option<String>,
    attempt_number: Option<u64>,
    attempt_count: u64,
    outcome_id: Option<String>,
    outcome_intent_id: Option<String>,
    outcome_attempt_id: Option<String>,
    outcome_run_id: Option<String>,
    outcome_status: Option<String>,
    outcome_consumed: Option<bool>,
    outcome_count: u64,
}

#[derive(Debug, PartialEq, Eq)]
struct ActiveLeaseLineage {
    effect_worker_id: Option<String>,
    effect_expires_at: Option<String>,
    checkpoint_worker_id: Option<String>,
    checkpoint_expires_at: Option<String>,
}

#[derive(Clone, Copy)]
enum CrashBoundary {
    ApprovedResumableBeforeClaim,
    PreIoClaimed,
    AttemptStarted,
    ResolvedWithoutOutcome,
}

struct CrashFixture {
    _temporary: tempfile::TempDir,
    workdir: PathBuf,
    database_path: PathBuf,
    agent: AgentSpec,
    principal_id: PrincipalId,
    service_principal_id: PrincipalId,
    approval_id: ApprovalId,
    effect_id: EffectId,
    run_id: RunId,
    conversation_id: ConversationId,
    turn_id: TurnId,
    step_id: StepId,
    initial_worker_id: WorkerId,
    seeded_attempt_id: Option<EffectAttemptId>,
    invocations: Arc<AtomicUsize>,
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
                recovery_lease: RECOVERY_LEASE,
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
                step.completed_at IS NOT NULL,
                (SELECT attempt.id FROM effect_attempts attempt
                 WHERE attempt.intent_id = effect.id ORDER BY attempt.attempt_number LIMIT 1),
                (SELECT attempt.intent_id FROM effect_attempts attempt
                 WHERE attempt.intent_id = effect.id ORDER BY attempt.attempt_number LIMIT 1),
                (SELECT attempt.worker_id FROM effect_attempts attempt
                 WHERE attempt.intent_id = effect.id ORDER BY attempt.attempt_number LIMIT 1),
                (SELECT attempt.attempt_number FROM effect_attempts attempt
                 WHERE attempt.intent_id = effect.id ORDER BY attempt.attempt_number LIMIT 1),
                (SELECT COUNT(*) FROM effect_attempts attempt
                 WHERE attempt.intent_id = effect.id),
                (SELECT outcome.id FROM effect_outcomes outcome
                 WHERE outcome.intent_id = effect.id LIMIT 1),
                (SELECT outcome.intent_id FROM effect_outcomes outcome
                 WHERE outcome.intent_id = effect.id LIMIT 1),
                (SELECT outcome.attempt_id FROM effect_outcomes outcome
                 WHERE outcome.intent_id = effect.id LIMIT 1),
                (SELECT outcome.run_id FROM effect_outcomes outcome
                 WHERE outcome.intent_id = effect.id LIMIT 1),
                (SELECT outcome.status FROM effect_outcomes outcome
                 WHERE outcome.intent_id = effect.id LIMIT 1),
                (SELECT outcome.consumed FROM effect_outcomes outcome
                 WHERE outcome.intent_id = effect.id LIMIT 1),
                (SELECT COUNT(*) FROM effect_outcomes outcome
                 WHERE outcome.intent_id = effect.id)
             FROM approval_requests approval
             JOIN effect_intents effect ON effect.id = approval.effect_id
             JOIN runs run ON run.id = approval.run_id
             JOIN steps step ON step.id = effect.step_id
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
                    step_completed: row.get(24)?,
                    attempt_id: row.get(25)?,
                    attempt_intent_id: row.get(26)?,
                    attempt_worker_id: row.get(27)?,
                    attempt_number: row.get(28)?,
                    attempt_count: row.get(29)?,
                    outcome_id: row.get(30)?,
                    outcome_intent_id: row.get(31)?,
                    outcome_attempt_id: row.get(32)?,
                    outcome_run_id: row.get(33)?,
                    outcome_status: row.get(34)?,
                    outcome_consumed: row.get(35)?,
                    outcome_count: row.get(36)?,
                })
            },
        )
        .expect("load exact APR-08 durable lineage")
}

fn active_lease_lineage(pool: &SqlitePool, effect_id: EffectId) -> ActiveLeaseLineage {
    pool.writer()
        .query_row(
            "SELECT effect.claimed_by, effect.claimed_until,
                    checkpoint.lease_owner, checkpoint.lease_expires_at
             FROM effect_intents effect
             JOIN execution_checkpoints checkpoint ON checkpoint.run_id = effect.run_id
             WHERE effect.id = ?1",
            [effect_id.to_string()],
            |row| {
                Ok(ActiveLeaseLineage {
                    effect_worker_id: row.get(0)?,
                    effect_expires_at: row.get(1)?,
                    checkpoint_worker_id: row.get(2)?,
                    checkpoint_expires_at: row.get(3)?,
                })
            },
        )
        .expect("load active effect/checkpoint lease lineage")
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

impl CrashFixture {
    async fn seed(boundary: CrashBoundary) -> Self {
        let temporary = tempfile::tempdir().expect("APR-08 temporary directory");
        let workdir = temporary.path().canonicalize().expect("canonical workdir");
        let database_path = workdir.join("approval-attempt-boundary.sqlite");
        let pool = open_migrated(&database_path);
        let store = SqliteRunStore::new(pool.clone());
        let agent_row = store
            .create_agent("apr08-attempt-boundary-agent", None, "{}")
            .expect("persist APR-08 recovery agent");
        let agent_id: AgentId = agent_row.id.parse().expect("typed agent ID");
        let mut agent = AgentSpec::new(agent_id, "apr08-attempt-boundary-agent", "fixture/model");
        agent.tools = vec![TOOL_NAME.to_owned()];
        let conversation_id = ConversationId::new();
        ConversationStore::create(&pool, Conversation::new(conversation_id, agent_id))
            .await
            .expect("persist APR-08 recovery conversation");
        let run_id = RunId::new();
        RunStore::create_correlated(
            &pool,
            run_id,
            &agent_id.to_string(),
            Some(&conversation_id.to_string()),
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
        let initial_worker_id = WorkerId::new();
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
            accumulated_usage: serde_json::to_value(TokenUsage::default())
                .expect("checkpoint usage"),
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
        let params_hash = IdempotencyKey::hash_params(
            &serde_json::to_vec(&arguments).expect("serialize effect arguments"),
        );
        let idempotency_key =
            IdempotencyKey::generate(run_id, 1, 0, EffectKind::ToolCall, params_hash).to_hex();
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
            effect_idempotency_key: idempotency_key,
            retry_class: StoreRetryClass::CheckBeforeRetry,
            checkpoint,
            metadata: ApprovalRequestMetadata {
                title: "Approve attempt-boundary fixture".to_owned(),
                description: "Allow this exact deterministic fixture once".to_owned(),
                reason: "APR-08 attempt-boundary recovery evidence".to_owned(),
                tenant_id: TENANT.to_owned(),
                workspace_id: WORKSPACE.to_owned(),
                authorized_principal_id: principal_id,
            },
            deadline_at: deadline,
        })
        .await
        .expect("pause exact APR-08 effect");
        let approval = pool
            .resolve_approval(ResolveApproval {
                approval_id,
                expected_run_state_version: 1,
                effect_id,
                run_id,
                turn_id,
                conversation_id,
                subject_digest: subject.subject_digest.clone(),
                scope: ApprovalScope {
                    tenant_id: TENANT.to_owned(),
                    workspace_id: WORKSPACE.to_owned(),
                    conversation_id: Some(conversation_id),
                    principal_id,
                },
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
        let _: TokenUsage = serde_json::from_value(checkpoint.checkpoint.accumulated_usage.clone())
            .expect("checkpoint token usage must round-trip");
        let _: Vec<InferenceMessage> =
            serde_json::from_value(checkpoint.checkpoint.messages.clone())
                .expect("checkpoint messages must round-trip");
        let claim = if matches!(boundary, CrashBoundary::ApprovedResumableBeforeClaim) {
            None
        } else {
            Some(
                pool.claim_approved_effect(ClaimApprovedEffect {
                    approval_id,
                    effect_id,
                    run_id,
                    subject_digest: subject.subject_digest,
                    expected_checkpoint_version: checkpoint.checkpoint.version,
                    worker_id: initial_worker_id,
                    lease_duration: SEEDED_CRASH_LEASE,
                })
                .await
                .expect("claim exact approved APR-08 effect"),
            )
        };
        let seeded_attempt_id = match boundary {
            CrashBoundary::ApprovedResumableBeforeClaim | CrashBoundary::PreIoClaimed => None,
            CrashBoundary::AttemptStarted | CrashBoundary::ResolvedWithoutOutcome => {
                let attempt_id = EffectAttemptId::new();
                pool.record_attempt_start(
                    attempt_id,
                    effect_id,
                    claim
                        .as_ref()
                        .expect("attempt boundary requires an effect claim")
                        .worker_id,
                    serde_json::json!({"boundary": "possible_io"}),
                )
                .await
                .expect("persist possible-I/O attempt boundary");
                Some(attempt_id)
            }
        };
        if matches!(boundary, CrashBoundary::ResolvedWithoutOutcome) {
            let attempt_id = seeded_attempt_id.expect("resolved boundary requires an attempt");
            let outcome_id = EffectOutcomeId::new();
            EffectStore::record_outcome(
                &pool,
                StoredOutcome {
                    id: outcome_id,
                    intent_id: effect_id,
                    attempt_id,
                    run_id,
                    consumed: false,
                    payload: serde_json::json!({
                        "variant": "success",
                        "data": {"executed": true},
                    }),
                    observed_at: Utc::now(),
                },
            )
            .await
            .expect("production outcome transaction must create the valid pair");
            let valid_pair = durable_lineage(&pool, approval_id);
            assert_eq!(valid_pair.effect_state, "resolved");
            assert_eq!(valid_pair.outcome_count, 1);
            assert_eq!(valid_pair.outcome_id, Some(outcome_id.to_string()));
            assert_eq!(valid_pair.outcome_attempt_id, Some(attempt_id.to_string()));

            let deleted = pool
                .writer()
                .execute(
                    "DELETE FROM effect_outcomes WHERE id = ?1 AND intent_id = ?2",
                    [outcome_id.to_string(), effect_id.to_string()],
                )
                .expect("inject explicit outcome-row corruption");
            assert_eq!(deleted, 1);
            let corrupted = durable_lineage(&pool, approval_id);
            assert_eq!(corrupted.effect_state, "resolved");
            assert_eq!(corrupted.outcome_count, 0);
        }
        let lease = active_lease_lineage(&pool, effect_id);
        match boundary {
            CrashBoundary::ApprovedResumableBeforeClaim => {
                assert_eq!(
                    lease,
                    ActiveLeaseLineage {
                        effect_worker_id: None,
                        effect_expires_at: None,
                        checkpoint_worker_id: None,
                        checkpoint_expires_at: None,
                    }
                );
            }
            CrashBoundary::PreIoClaimed | CrashBoundary::AttemptStarted => {
                let expected_worker_id = initial_worker_id.to_string();
                assert_eq!(
                    lease.effect_worker_id.as_deref(),
                    Some(expected_worker_id.as_str())
                );
                assert_eq!(
                    lease.checkpoint_worker_id.as_deref(),
                    Some(expected_worker_id.as_str())
                );
                assert!(lease.effect_expires_at.is_some());
                assert!(lease.checkpoint_expires_at.is_some());
            }
            CrashBoundary::ResolvedWithoutOutcome => {
                let expected_worker_id = initial_worker_id.to_string();
                assert!(lease.effect_worker_id.is_none());
                assert!(lease.effect_expires_at.is_none());
                assert_eq!(
                    lease.checkpoint_worker_id.as_deref(),
                    Some(expected_worker_id.as_str())
                );
                assert!(lease.checkpoint_expires_at.is_some());
            }
        }
        assert_eq!(invocations.load(Ordering::SeqCst), 0);

        drop(original_service);
        drop(store);
        drop(pool);
        Self {
            _temporary: temporary,
            workdir,
            database_path,
            agent,
            principal_id,
            service_principal_id,
            approval_id,
            effect_id,
            run_id,
            conversation_id,
            turn_id,
            step_id,
            initial_worker_id,
            seeded_attempt_id,
            invocations,
        }
    }

    fn open_pool(&self) -> SqlitePool {
        open_migrated(&self.database_path)
    }

    fn restarted_service(&self, pool: &SqlitePool) -> AppService {
        build_service(
            pool,
            &self.agent,
            self.principal_id,
            self.service_principal_id,
            &self.workdir,
            Arc::clone(&self.invocations),
        )
    }

    fn assert_exact_identity(&self, lineage: &DurableLineage) {
        let agent_id = self.agent.id.to_string();
        let approval_id = self.approval_id.to_string();
        let effect_id = self.effect_id.to_string();
        let run_id = self.run_id.to_string();
        let conversation_id = self.conversation_id.to_string();
        let turn_id = self.turn_id.to_string();
        let step_id = self.step_id.to_string();
        assert_eq!(lineage.approval_id, approval_id);
        assert_eq!(lineage.approval_effect_id, effect_id);
        assert_eq!(lineage.approval_run_id, run_id);
        assert_eq!(lineage.approval_turn_id, turn_id);
        assert_eq!(lineage.approval_conversation_id, conversation_id);
        assert_eq!(lineage.approval_agent_id, agent_id);
        assert_eq!(lineage.effect_id, effect_id);
        assert_eq!(lineage.effect_run_id, run_id);
        assert_eq!(lineage.effect_turn_id, turn_id);
        assert_eq!(lineage.effect_step_id, step_id);
        assert_eq!(
            lineage.effect_approval_id.as_deref(),
            Some(approval_id.as_str())
        );
        assert_eq!(lineage.run_id, run_id);
        assert_eq!(lineage.run_agent_id, agent_id);
        assert_eq!(
            lineage.run_conversation_id.as_deref(),
            Some(conversation_id.as_str())
        );
        assert_eq!(lineage.checkpoint_run_id, run_id);
        assert_eq!(lineage.checkpoint_turn_id, turn_id);
        assert_eq!(lineage.checkpoint_conversation_id, conversation_id);
        assert_eq!(lineage.checkpoint_agent_id, agent_id);
    }

    async fn wait_for_expired_claim(&self) {
        tokio::time::sleep(SEEDED_CRASH_LEASE.saturating_mul(3)).await;
        let pool = self.open_pool();
        let lease = active_lease_lineage(&pool, self.effect_id);
        let now = Utc::now();
        let effect_expiry = chrono::DateTime::parse_from_rfc3339(
            lease
                .effect_expires_at
                .as_deref()
                .expect("claimed effect must retain its lease expiry"),
        )
        .expect("parse effect lease expiry")
        .with_timezone(&Utc);
        let checkpoint_expiry = chrono::DateTime::parse_from_rfc3339(
            lease
                .checkpoint_expires_at
                .as_deref()
                .expect("leased checkpoint must retain its lease expiry"),
        )
        .expect("parse checkpoint lease expiry")
        .with_timezone(&Utc);
        let expected_worker_id = self.initial_worker_id.to_string();
        assert_eq!(
            lease.effect_worker_id.as_deref(),
            Some(expected_worker_id.as_str())
        );
        assert_eq!(
            lease.checkpoint_worker_id.as_deref(),
            Some(expected_worker_id.as_str())
        );
        assert!(effect_expiry <= now);
        assert!(checkpoint_expiry <= now);
    }

    async fn wait_for_expired_checkpoint_without_effect_lease(&self) {
        tokio::time::sleep(SEEDED_CRASH_LEASE.saturating_mul(3)).await;
        let pool = self.open_pool();
        let lease = active_lease_lineage(&pool, self.effect_id);
        let checkpoint_expiry = chrono::DateTime::parse_from_rfc3339(
            lease
                .checkpoint_expires_at
                .as_deref()
                .expect("corrupt resolved boundary retains the checkpoint lease expiry"),
        )
        .expect("parse checkpoint lease expiry")
        .with_timezone(&Utc);
        let expected_worker_id = self.initial_worker_id.to_string();
        assert!(lease.effect_worker_id.is_none());
        assert!(lease.effect_expires_at.is_none());
        assert_eq!(
            lease.checkpoint_worker_id.as_deref(),
            Some(expected_worker_id.as_str())
        );
        assert!(checkpoint_expiry <= Utc::now());
    }
}

#[tokio::test]
async fn approved_resumable_effect_before_claim_executes_once_across_restarts() {
    let fixture = CrashFixture::seed(CrashBoundary::ApprovedResumableBeforeClaim).await;
    let before_pool = fixture.open_pool();
    let before_crash = durable_lineage(&before_pool, fixture.approval_id);
    fixture.assert_exact_identity(&before_crash);
    assert_eq!(before_crash.approval_status, "approved");
    assert_eq!(before_crash.effect_state, "approved");
    assert_eq!(
        before_crash.run_state,
        format!("waiting_effect:{}", fixture.effect_id)
    );
    assert_eq!(before_crash.run_state_version, 2);
    assert_eq!(before_crash.checkpoint_version, 2);
    assert_eq!(before_crash.checkpoint_status, "resumable");
    assert!(!before_crash.step_completed);
    assert_eq!(before_crash.attempt_count, 0);
    assert_eq!(before_crash.outcome_count, 0);
    assert_eq!(fixture.invocations.load(Ordering::SeqCst), 0);
    assert_eq!(
        active_lease_lineage(&before_pool, fixture.effect_id),
        ActiveLeaseLineage {
            effect_worker_id: None,
            effect_expires_at: None,
            checkpoint_worker_id: None,
            checkpoint_expires_at: None,
        }
    );
    drop(before_pool);

    let first_pool = fixture.open_pool();
    let first_restart = fixture.restarted_service(&first_pool);
    let recovered = first_restart
        .recover_approval_checkpoints()
        .await
        .expect("approved resumable recovery must complete");
    assert_eq!(recovered, 1);
    assert_eq!(fixture.invocations.load(Ordering::SeqCst), 1);
    let after_recovery = durable_lineage(&first_pool, fixture.approval_id);
    fixture.assert_exact_identity(&after_recovery);
    assert_eq!(after_recovery.approval_status, "approved");
    assert_eq!(after_recovery.effect_state, "resolved");
    assert_eq!(after_recovery.run_state, "completed");
    assert_eq!(after_recovery.run_state_version, 3);
    assert_eq!(after_recovery.checkpoint_status, "terminal");
    assert_eq!(after_recovery.checkpoint_version, 3);
    assert!(after_recovery.step_completed);
    assert_eq!(after_recovery.attempt_count, 1);
    assert_eq!(after_recovery.attempt_number, Some(1));
    let effect_id = fixture.effect_id.to_string();
    let run_id = fixture.run_id.to_string();
    assert_eq!(
        after_recovery.attempt_intent_id.as_deref(),
        Some(effect_id.as_str())
    );
    assert!(after_recovery.attempt_worker_id.is_some());
    let attempt_id = after_recovery
        .attempt_id
        .as_deref()
        .expect("recovery persists one exact attempt");
    assert_eq!(after_recovery.outcome_count, 1);
    assert_eq!(
        after_recovery.outcome_intent_id.as_deref(),
        Some(effect_id.as_str())
    );
    assert_eq!(
        after_recovery.outcome_attempt_id.as_deref(),
        Some(attempt_id)
    );
    assert_eq!(
        after_recovery.outcome_run_id.as_deref(),
        Some(run_id.as_str())
    );
    assert_eq!(after_recovery.outcome_status.as_deref(), Some("success"));
    assert_eq!(after_recovery.outcome_consumed, Some(true));
    assert!(after_recovery.outcome_id.is_some());
    drop(first_restart);
    drop(first_pool);

    let second_pool = fixture.open_pool();
    let second_restart = fixture.restarted_service(&second_pool);
    let recovered_again = second_restart
        .recover_approval_checkpoints()
        .await
        .expect("terminal checkpoint recovery is idempotent");
    assert_eq!(recovered_again, 0);
    assert_eq!(fixture.invocations.load(Ordering::SeqCst), 1);
    assert_eq!(
        durable_lineage(&second_pool, fixture.approval_id),
        after_recovery
    );
}

#[tokio::test]
async fn claimed_effect_before_attempt_is_reclaimed_and_executes_once_across_restarts() {
    let fixture = CrashFixture::seed(CrashBoundary::PreIoClaimed).await;
    let before_pool = fixture.open_pool();
    let before_crash = durable_lineage(&before_pool, fixture.approval_id);
    fixture.assert_exact_identity(&before_crash);
    assert_eq!(before_crash.approval_status, "approved");
    assert_eq!(before_crash.effect_state, "claimed");
    assert_eq!(
        before_crash.run_state,
        format!("waiting_effect:{}", fixture.effect_id)
    );
    assert_eq!(before_crash.run_state_version, 2);
    assert_eq!(before_crash.checkpoint_version, 2);
    assert_eq!(before_crash.checkpoint_status, "leased");
    assert!(!before_crash.step_completed);
    assert_eq!(before_crash.attempt_count, 0);
    assert_eq!(before_crash.outcome_count, 0);
    drop(before_pool);
    fixture.wait_for_expired_claim().await;

    let first_pool = fixture.open_pool();
    let first_restart = fixture.restarted_service(&first_pool);
    let recovered = first_restart
        .recover_approval_checkpoints()
        .await
        .expect("pre-I/O claim recovery must complete");
    assert_eq!(recovered, 1);
    assert_eq!(fixture.invocations.load(Ordering::SeqCst), 1);
    let after_recovery = durable_lineage(&first_pool, fixture.approval_id);
    fixture.assert_exact_identity(&after_recovery);
    assert_eq!(after_recovery.approval_status, "approved");
    assert_eq!(after_recovery.effect_state, "resolved");
    assert_eq!(after_recovery.run_state, "completed");
    assert_eq!(after_recovery.run_state_version, 3);
    assert_eq!(after_recovery.checkpoint_status, "terminal");
    assert_eq!(after_recovery.checkpoint_version, 3);
    assert!(after_recovery.step_completed);
    assert_eq!(after_recovery.attempt_count, 1);
    assert_eq!(after_recovery.attempt_number, Some(1));
    let effect_id = fixture.effect_id.to_string();
    let initial_worker_id = fixture.initial_worker_id.to_string();
    let run_id = fixture.run_id.to_string();
    assert_eq!(
        after_recovery.attempt_intent_id.as_deref(),
        Some(effect_id.as_str())
    );
    let recovery_worker_id = after_recovery
        .attempt_worker_id
        .as_deref()
        .expect("recovery attempt preserves its exact worker identity");
    assert_ne!(recovery_worker_id, initial_worker_id);
    let attempt_id = after_recovery
        .attempt_id
        .as_deref()
        .expect("recovery persists one exact attempt");
    assert_eq!(after_recovery.outcome_count, 1);
    assert_eq!(
        after_recovery.outcome_intent_id.as_deref(),
        Some(effect_id.as_str())
    );
    assert_eq!(
        after_recovery.outcome_attempt_id.as_deref(),
        Some(attempt_id)
    );
    assert_eq!(
        after_recovery.outcome_run_id.as_deref(),
        Some(run_id.as_str())
    );
    assert_eq!(after_recovery.outcome_status.as_deref(), Some("success"));
    assert_eq!(after_recovery.outcome_consumed, Some(true));
    assert!(after_recovery.outcome_id.is_some());
    drop(first_restart);
    drop(first_pool);

    let second_pool = fixture.open_pool();
    let second_restart = fixture.restarted_service(&second_pool);
    let recovered_again = second_restart
        .recover_approval_checkpoints()
        .await
        .expect("terminal checkpoint recovery is idempotent");
    assert_eq!(recovered_again, 0);
    assert_eq!(fixture.invocations.load(Ordering::SeqCst), 1);
    assert_eq!(
        durable_lineage(&second_pool, fixture.approval_id),
        after_recovery
    );
}

#[tokio::test]
async fn executing_approved_effect_without_outcome_fails_closed_across_restarts() {
    let fixture = CrashFixture::seed(CrashBoundary::AttemptStarted).await;
    let before_pool = fixture.open_pool();
    let before_crash = durable_lineage(&before_pool, fixture.approval_id);
    fixture.assert_exact_identity(&before_crash);
    assert_eq!(before_crash.approval_status, "approved");
    assert_eq!(before_crash.effect_state, "executing");
    assert_eq!(
        before_crash.run_state,
        format!("waiting_effect:{}", fixture.effect_id)
    );
    assert_eq!(before_crash.run_state_version, 2);
    assert_eq!(before_crash.checkpoint_version, 2);
    assert_eq!(before_crash.checkpoint_status, "leased");
    assert!(!before_crash.step_completed);
    assert_eq!(
        before_crash.attempt_id,
        fixture.seeded_attempt_id.map(|id| id.to_string())
    );
    assert_eq!(before_crash.attempt_count, 1);
    assert_eq!(before_crash.outcome_count, 0);
    assert_eq!(fixture.invocations.load(Ordering::SeqCst), 0);
    drop(before_pool);
    fixture.wait_for_expired_claim().await;

    let first_pool = fixture.open_pool();
    let first_restart = fixture.restarted_service(&first_pool);
    let first_error = first_restart
        .recover_approval_checkpoints()
        .await
        .expect_err("possible-I/O recovery must fail closed");
    assert_manual_reconciliation(first_error, fixture.run_id, fixture.effect_id);
    assert_eq!(fixture.invocations.load(Ordering::SeqCst), 0);
    assert_eq!(
        durable_lineage(&first_pool, fixture.approval_id),
        before_crash
    );
    drop(first_restart);
    drop(first_pool);
    tokio::time::sleep(RECOVERY_LEASE.saturating_mul(3)).await;

    let second_pool = fixture.open_pool();
    let second_restart = fixture.restarted_service(&second_pool);
    let second_error = second_restart
        .recover_approval_checkpoints()
        .await
        .expect_err("second recovery must preserve manual reconciliation");
    assert_manual_reconciliation(second_error, fixture.run_id, fixture.effect_id);
    assert_eq!(fixture.invocations.load(Ordering::SeqCst), 0);
    assert_eq!(
        durable_lineage(&second_pool, fixture.approval_id),
        before_crash
    );
}

#[tokio::test]
async fn corrupted_resolved_effect_without_outcome_fails_closed_across_restarts() {
    let fixture = CrashFixture::seed(CrashBoundary::ResolvedWithoutOutcome).await;
    let before_pool = fixture.open_pool();
    let before_crash = durable_lineage(&before_pool, fixture.approval_id);
    fixture.assert_exact_identity(&before_crash);
    assert_eq!(before_crash.approval_status, "approved");
    assert_eq!(before_crash.effect_state, "resolved");
    assert_eq!(
        before_crash.run_state,
        format!("waiting_effect:{}", fixture.effect_id)
    );
    assert_eq!(before_crash.run_state_version, 2);
    assert_eq!(before_crash.checkpoint_version, 2);
    assert_eq!(before_crash.checkpoint_status, "leased");
    assert!(!before_crash.step_completed);
    assert_eq!(
        before_crash.attempt_id,
        fixture.seeded_attempt_id.map(|id| id.to_string())
    );
    assert_eq!(before_crash.attempt_count, 1);
    assert_eq!(before_crash.outcome_count, 0);
    assert_eq!(fixture.invocations.load(Ordering::SeqCst), 0);
    let lease = active_lease_lineage(&before_pool, fixture.effect_id);
    let expected_worker_id = fixture.initial_worker_id.to_string();
    assert!(lease.effect_worker_id.is_none());
    assert!(lease.effect_expires_at.is_none());
    assert_eq!(
        lease.checkpoint_worker_id.as_deref(),
        Some(expected_worker_id.as_str())
    );
    assert!(lease.checkpoint_expires_at.is_some());
    drop(before_pool);
    fixture
        .wait_for_expired_checkpoint_without_effect_lease()
        .await;

    let first_pool = fixture.open_pool();
    let first_restart = fixture.restarted_service(&first_pool);
    let first_error = first_restart
        .recover_approval_checkpoints()
        .await
        .expect_err("resolved-without-outcome recovery must fail closed");
    assert_manual_reconciliation(first_error, fixture.run_id, fixture.effect_id);
    assert_eq!(fixture.invocations.load(Ordering::SeqCst), 0);
    assert_eq!(
        durable_lineage(&first_pool, fixture.approval_id),
        before_crash
    );
    drop(first_restart);
    drop(first_pool);
    tokio::time::sleep(RECOVERY_LEASE.saturating_mul(3)).await;

    let second_pool = fixture.open_pool();
    let second_restart = fixture.restarted_service(&second_pool);
    let second_error = second_restart
        .recover_approval_checkpoints()
        .await
        .expect_err("second recovery must preserve resolved manual reconciliation");
    assert_manual_reconciliation(second_error, fixture.run_id, fixture.effect_id);
    assert_eq!(fixture.invocations.load(Ordering::SeqCst), 0);
    assert_eq!(
        durable_lineage(&second_pool, fixture.approval_id),
        before_crash
    );
}
