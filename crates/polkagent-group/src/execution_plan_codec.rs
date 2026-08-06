//! Canonical durable encoding for group execution plans.
//!
//! The derived serde representation of [`ExecutionPlan`] is intentionally not
//! a persistence protocol. This module owns the validated, versioned v1 DTO,
//! exact canonical JSON bytes, and domain-separated digest used by future
//! durable ledger work.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

use polkagent_core::ids::AgentId;
use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Number, Value};
use thiserror::Error;

use crate::execution::{ExecutionMode, ExecutionPlan, GroupTask, TaskId};
use crate::types::GrantSpec;

/// Stable contract discriminator for durable execution-plan v1 bytes.
pub const EXECUTION_PLAN_V1_CONTRACT: &str = "polkagent.group-execution-plan";
/// Stable schema version for the first durable execution-plan contract.
pub const EXECUTION_PLAN_V1_SCHEMA_VERSION: u64 = 1;
/// Stable digest prefix for v1's BLAKE3 digest.
pub const EXECUTION_PLAN_V1_DIGEST_PREFIX: &str = "blake3-v1:";
/// Maximum number of tasks accepted by one v1 plan.
pub const EXECUTION_PLAN_V1_MAX_TASKS: usize = 1_024;
/// Maximum number of dependency edges accepted by one v1 plan.
pub const EXECUTION_PLAN_V1_MAX_DEPENDENCY_EDGES: usize = 16_384;
/// Maximum number of bytes in one task's canonical JSON input.
pub const EXECUTION_PLAN_V1_MAX_TASK_INPUT_BYTES: usize = 262_144;
/// Maximum nesting depth in one task input; a scalar has depth zero.
pub const EXECUTION_PLAN_V1_MAX_TASK_INPUT_DEPTH: usize = 64;
/// Maximum number of bytes in one complete canonical v1 plan.
pub const EXECUTION_PLAN_V1_MAX_CANONICAL_BYTES: usize = 1_048_576;

const DIGEST_DOMAIN: &[u8] = b"polkagent.group-execution-plan.v1";

/// A validated v1 grant override in its durable normalized representation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableGrantSpecV1 {
    capabilities: Vec<String>,
    max_budget: Option<String>,
    allowed_pallets: Vec<String>,
}

impl DurableGrantSpecV1 {
    /// Sorted, unique capability names.
    pub fn capabilities(&self) -> &[String] {
        &self.capabilities
    }

    /// Canonical Ryū budget text, including `"0"` for either signed zero.
    pub fn max_budget(&self) -> Option<&str> {
        self.max_budget.as_deref()
    }

    /// Sorted, unique pallet names.
    pub fn allowed_pallets(&self) -> &[String] {
        &self.allowed_pallets
    }
}

/// A validated declaration-ordered task in a durable v1 plan.
#[derive(Debug, Clone, PartialEq)]
pub struct DurableGroupTaskV1 {
    ordinal: u32,
    task_id: TaskId,
    agent_id: AgentId,
    input: Value,
    grant_override: Option<DurableGrantSpecV1>,
}

impl DurableGroupTaskV1 {
    /// Zero-based declaration ordinal.
    pub fn ordinal(&self) -> u32 {
        self.ordinal
    }

    /// Full-range task identifier.
    pub fn task_id(&self) -> TaskId {
        self.task_id
    }

    /// Canonical unsigned decimal task identifier used in JSON and storage.
    pub fn task_id_decimal(&self) -> String {
        self.task_id.value().to_string()
    }

    /// Agent selected to execute this task.
    pub fn agent_id(&self) -> AgentId {
        self.agent_id
    }

    /// Validated JSON input. Object key order is fixed by the encoder.
    pub fn input(&self) -> &Value {
        &self.input
    }

    /// Optional normalized task grant override.
    pub fn grant_override(&self) -> Option<&DurableGrantSpecV1> {
        self.grant_override.as_ref()
    }
}

/// A validated dependency edge in numeric `(blocked_by, task_id)` order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DurableDependencyEdgeV1 {
    blocked_by: TaskId,
    task_id: TaskId,
}

impl DurableDependencyEdgeV1 {
    /// Task that must complete first.
    pub fn blocked_by(&self) -> TaskId {
        self.blocked_by
    }

    /// Task that is blocked by the prerequisite.
    pub fn task_id(&self) -> TaskId {
        self.task_id
    }

    /// Canonical decimal form of the prerequisite ID.
    pub fn blocked_by_decimal(&self) -> String {
        self.blocked_by.value().to_string()
    }

    /// Canonical decimal form of the blocked task ID.
    pub fn task_id_decimal(&self) -> String {
        self.task_id.value().to_string()
    }
}

/// The validated, normalized durable execution-plan v1 domain DTO.
#[derive(Debug, Clone, PartialEq)]
pub struct DurableExecutionPlanV1 {
    mode: ExecutionMode,
    tasks: Vec<DurableGroupTaskV1>,
    dependency_edges: Vec<DurableDependencyEdgeV1>,
}

impl DurableExecutionPlanV1 {
    /// Execution strategy retained from the in-memory plan.
    pub fn mode(&self) -> ExecutionMode {
        self.mode
    }

    /// Tasks in declaration order.
    pub fn tasks(&self) -> &[DurableGroupTaskV1] {
        &self.tasks
    }

    /// Dependency edges in numeric `(blocked_by, task_id)` order.
    pub fn dependency_edges(&self) -> &[DurableDependencyEdgeV1] {
        &self.dependency_edges
    }

    /// Upcast this validated normalized DTO without losing accepted v1 data.
    ///
    /// Encoding intentionally normalizes duplicate grant-set entries and both
    /// IEEE-754 signed-zero values, so an upcast preserves the accepted v1
    /// semantics rather than reconstructing those pre-normalization details.
    pub fn to_execution_plan(&self) -> Result<ExecutionPlan, ExecutionPlanCodecError> {
        let tasks = self
            .tasks
            .iter()
            .map(|task| {
                let grant_spec = task
                    .grant_override
                    .as_ref()
                    .map(grant_to_domain)
                    .transpose()?;
                Ok(GroupTask {
                    id: task.task_id,
                    agent_id: task.agent_id,
                    input: task.input.clone(),
                    grant_spec,
                })
            })
            .collect::<Result<Vec<_>, ExecutionPlanCodecError>>()?;
        let mut dependencies = HashMap::<TaskId, Vec<TaskId>>::new();
        for edge in &self.dependency_edges {
            dependencies
                .entry(edge.blocked_by)
                .or_default()
                .push(edge.task_id);
        }
        Ok(ExecutionPlan {
            mode: self.mode,
            tasks,
            dependencies,
        })
    }
}

impl TryFrom<&DurableExecutionPlanV1> for ExecutionPlan {
    type Error = ExecutionPlanCodecError;

    fn try_from(value: &DurableExecutionPlanV1) -> Result<Self, Self::Error> {
        value.to_execution_plan()
    }
}

/// A validated v1 DTO bound to its exact canonical bytes and digest.
#[derive(Debug, Clone, PartialEq)]
pub struct CanonicalExecutionPlanV1 {
    plan: DurableExecutionPlanV1,
    canonical_bytes: Vec<u8>,
    digest: String,
}

impl CanonicalExecutionPlanV1 {
    /// Validated durable plan DTO.
    pub fn plan(&self) -> &DurableExecutionPlanV1 {
        &self.plan
    }

    /// Exact canonical JSON bytes, with no BOM or trailing newline.
    pub fn canonical_bytes(&self) -> &[u8] {
        &self.canonical_bytes
    }

    /// Domain-separated `blake3-v1:` digest of the exact canonical bytes.
    pub fn digest(&self) -> &str {
        &self.digest
    }

    /// Upcast the validated normalized DTO without losing accepted v1 data.
    pub fn to_execution_plan(&self) -> Result<ExecutionPlan, ExecutionPlanCodecError> {
        self.plan.to_execution_plan()
    }
}

/// Fail-closed errors returned by the durable execution-plan codec.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ExecutionPlanCodecError {
    /// A plan must contain at least one task.
    #[error("execution plan must contain at least one task")]
    EmptyPlan,
    /// The stable v1 task count limit was exceeded.
    #[error("execution plan has {actual} tasks; maximum is {maximum}")]
    TooManyTasks {
        /// Observed count.
        actual: usize,
        /// Stable v1 maximum.
        maximum: usize,
    },
    /// The stable v1 edge count limit was exceeded.
    #[error("execution plan has {actual} dependency edges; maximum is {maximum}")]
    TooManyDependencyEdges {
        /// Observed count.
        actual: usize,
        /// Stable v1 maximum.
        maximum: usize,
    },
    /// The stable complete-plan byte limit was exceeded.
    #[error("canonical execution plan has {actual} bytes; maximum is {maximum}")]
    PlanTooLarge {
        /// Observed bytes.
        actual: usize,
        /// Stable v1 maximum.
        maximum: usize,
    },
    /// One canonical task input exceeded its byte limit.
    #[error("task ordinal {ordinal} input has {actual} bytes; maximum is {maximum}")]
    TaskInputTooLarge {
        /// Task declaration ordinal.
        ordinal: usize,
        /// Observed bytes.
        actual: usize,
        /// Stable v1 maximum.
        maximum: usize,
    },
    /// One task input exceeded its nesting depth limit.
    #[error("task ordinal {ordinal} input exceeds maximum JSON depth {maximum}")]
    TaskInputTooDeep {
        /// Task declaration ordinal.
        ordinal: usize,
        /// Stable v1 maximum.
        maximum: usize,
    },
    /// Task input contained a floating-point JSON number.
    #[error("task ordinal {ordinal} input contains a non-integer JSON number")]
    NonIntegerTaskInput {
        /// Task declaration ordinal.
        ordinal: usize,
    },
    /// Task IDs must be unique within a plan.
    #[error("duplicate task ID {0}")]
    DuplicateTaskId(u64),
    /// A decoded ordinal did not exactly match declaration order.
    #[error("task at position {position} has ordinal {actual}; expected {expected}")]
    InvalidOrdinal {
        /// Vector position.
        position: usize,
        /// Required ordinal.
        expected: u32,
        /// Decoded ordinal.
        actual: u32,
    },
    /// A durable task identifier was not canonical full-range decimal text.
    #[error("invalid canonical task ID")]
    InvalidTaskId,
    /// A dependency endpoint did not name a declared task.
    #[error("dependency edge {blocked_by} -> {task_id} has a dangling endpoint")]
    DanglingDependency {
        /// Prerequisite task ID.
        blocked_by: u64,
        /// Blocked task ID.
        task_id: u64,
    },
    /// Self dependencies are invalid.
    #[error("task {0} cannot depend on itself")]
    SelfDependency(u64),
    /// Repeated dependency edges are invalid.
    #[error("duplicate dependency edge {blocked_by} -> {task_id}")]
    DuplicateDependency {
        /// Prerequisite task ID.
        blocked_by: u64,
        /// Blocked task ID.
        task_id: u64,
    },
    /// The dependency graph contains a cycle.
    #[error("dependency graph contains a cycle")]
    DependencyCycle,
    /// Consensus requires at least two independently declared tasks.
    #[error("consensus execution requires at least two tasks")]
    ConsensusRequiresTwoTasks,
    /// A capability or pallet set contained an empty entry.
    #[error("grant field `{field}` contains an empty string")]
    EmptyGrantEntry {
        /// Grant field name.
        field: &'static str,
    },
    /// A decoded grant set was not already sorted and unique.
    #[error("grant field `{field}` is not sorted and unique")]
    NonCanonicalGrantSet {
        /// Grant field name.
        field: &'static str,
    },
    /// A grant budget was negative, non-finite, or noncanonical.
    #[error("grant maximum budget is invalid or noncanonical")]
    InvalidGrantBudget,
    /// A duplicate object key was rejected before constructing a JSON value.
    #[error("execution-plan JSON contains a duplicate object key")]
    DuplicateJsonKey,
    /// The JSON byte stream was syntactically invalid.
    #[error("invalid execution-plan JSON at line {line}, column {column}")]
    InvalidJson {
        /// One-based line reported by the JSON parser, or zero when unavailable.
        line: usize,
        /// One-based column reported by the JSON parser, or zero when unavailable.
        column: usize,
    },
    /// A decoded JSON value did not match the closed v1 DTO shape.
    #[error("execution-plan JSON does not match the v1 structure")]
    InvalidStructure,
    /// A required version-envelope field was missing or had the wrong type.
    #[error("missing or invalid execution-plan envelope field `{0}`")]
    InvalidEnvelopeField(&'static str),
    /// The contract discriminator was not the accepted contract.
    #[error("unsupported execution-plan contract")]
    UnsupportedContract,
    /// Future versions fail closed until an explicit decoder is added.
    #[error("unsupported execution-plan schema version {0}")]
    UnsupportedVersion(u64),
    /// Digest text was not in the exact accepted format.
    #[error("invalid v1 execution-plan digest format")]
    InvalidDigestFormat,
    /// The supplied digest did not bind the supplied exact bytes.
    #[error("execution-plan digest mismatch")]
    DigestMismatch,
    /// Valid JSON did not use the one accepted canonical byte representation.
    #[error("execution-plan JSON bytes are not canonical v1 bytes")]
    NonCanonicalBytes,
}

/// Validate and encode an in-memory execution plan as canonical durable v1.
pub fn encode_execution_plan_v1(
    plan: &ExecutionPlan,
) -> Result<CanonicalExecutionPlanV1, ExecutionPlanCodecError> {
    let durable = durable_from_domain(plan)?;
    freeze(durable)
}

/// Verify, decode, and validate exact canonical v1 bytes.
pub fn decode_execution_plan_v1(
    canonical_bytes: &[u8],
    expected_digest: &str,
) -> Result<CanonicalExecutionPlanV1, ExecutionPlanCodecError> {
    if canonical_bytes.len() > EXECUTION_PLAN_V1_MAX_CANONICAL_BYTES {
        return Err(ExecutionPlanCodecError::PlanTooLarge {
            actual: canonical_bytes.len(),
            maximum: EXECUTION_PLAN_V1_MAX_CANONICAL_BYTES,
        });
    }
    validate_digest_text(expected_digest)?;
    let actual_digest = compute_execution_plan_digest_v1(canonical_bytes);
    if !constant_time_eq(expected_digest.as_bytes(), actual_digest.as_bytes()) {
        return Err(ExecutionPlanCodecError::DigestMismatch);
    }

    let value = decode_without_duplicate_keys(canonical_bytes)?;
    let object = value
        .as_object()
        .ok_or(ExecutionPlanCodecError::InvalidEnvelopeField("contract"))?;
    let contract = object
        .get("contract")
        .and_then(Value::as_str)
        .ok_or(ExecutionPlanCodecError::InvalidEnvelopeField("contract"))?;
    if contract != EXECUTION_PLAN_V1_CONTRACT {
        return Err(ExecutionPlanCodecError::UnsupportedContract);
    }
    let schema_version = object.get("schema_version").and_then(Value::as_u64).ok_or(
        ExecutionPlanCodecError::InvalidEnvelopeField("schema_version"),
    )?;
    if schema_version != EXECUTION_PLAN_V1_SCHEMA_VERSION {
        return Err(ExecutionPlanCodecError::UnsupportedVersion(schema_version));
    }

    let wire: WirePlan =
        serde_json::from_value(value).map_err(|_| ExecutionPlanCodecError::InvalidStructure)?;
    let durable = durable_from_wire(wire)?;
    let frozen = freeze(durable)?;
    if frozen.canonical_bytes != canonical_bytes {
        return Err(ExecutionPlanCodecError::NonCanonicalBytes);
    }
    Ok(frozen)
}

/// Compute the domain-separated v1 digest for exact bytes.
#[must_use]
pub fn compute_execution_plan_digest_v1(canonical_bytes: &[u8]) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(DIGEST_DOMAIN);
    hasher.update(&[0]);
    hasher.update(canonical_bytes);
    format!(
        "{EXECUTION_PLAN_V1_DIGEST_PREFIX}{}",
        hasher.finalize().to_hex()
    )
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (a, b)| difference | (a ^ b))
        == 0
}

fn validate_digest_text(digest: &str) -> Result<(), ExecutionPlanCodecError> {
    let Some(hex) = digest.strip_prefix(EXECUTION_PLAN_V1_DIGEST_PREFIX) else {
        return Err(ExecutionPlanCodecError::InvalidDigestFormat);
    };
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(ExecutionPlanCodecError::InvalidDigestFormat);
    }
    Ok(())
}

fn durable_from_domain(
    plan: &ExecutionPlan,
) -> Result<DurableExecutionPlanV1, ExecutionPlanCodecError> {
    validate_task_count(plan.tasks.len(), plan.mode)?;
    let mut task_ids = HashSet::with_capacity(plan.tasks.len());
    let mut tasks = Vec::with_capacity(plan.tasks.len());
    for (ordinal, task) in plan.tasks.iter().enumerate() {
        if !task_ids.insert(task.id) {
            return Err(ExecutionPlanCodecError::DuplicateTaskId(task.id.value()));
        }
        validate_input(&task.input, ordinal)?;
        let grant_override = task.grant_spec.as_ref().map(normalize_grant).transpose()?;
        let ordinal =
            u32::try_from(ordinal).map_err(|_| ExecutionPlanCodecError::TooManyTasks {
                actual: plan.tasks.len(),
                maximum: EXECUTION_PLAN_V1_MAX_TASKS,
            })?;
        tasks.push(DurableGroupTaskV1 {
            ordinal,
            task_id: task.id,
            agent_id: task.agent_id,
            input: task.input.clone(),
            grant_override,
        });
    }

    let edge_count = plan.dependencies.values().map(Vec::len).sum::<usize>();
    validate_edge_count(edge_count)?;
    let mut dependency_edges = Vec::with_capacity(edge_count);
    let mut edge_set = HashSet::with_capacity(edge_count);
    for (blocked_by, blocked_tasks) in &plan.dependencies {
        for task_id in blocked_tasks {
            validate_edge(*blocked_by, *task_id, &task_ids, &mut edge_set)?;
            dependency_edges.push(DurableDependencyEdgeV1 {
                blocked_by: *blocked_by,
                task_id: *task_id,
            });
        }
    }
    dependency_edges.sort_unstable_by_key(|edge| (edge.blocked_by.value(), edge.task_id.value()));
    validate_acyclic(&task_ids, &dependency_edges)?;

    Ok(DurableExecutionPlanV1 {
        mode: plan.mode,
        tasks,
        dependency_edges,
    })
}

fn durable_from_wire(wire: WirePlan) -> Result<DurableExecutionPlanV1, ExecutionPlanCodecError> {
    if wire.contract != EXECUTION_PLAN_V1_CONTRACT {
        return Err(ExecutionPlanCodecError::UnsupportedContract);
    }
    if wire.schema_version != EXECUTION_PLAN_V1_SCHEMA_VERSION {
        return Err(ExecutionPlanCodecError::UnsupportedVersion(
            wire.schema_version,
        ));
    }
    validate_task_count(wire.tasks.len(), wire.mode)?;
    let mut task_ids = HashSet::with_capacity(wire.tasks.len());
    let mut tasks = Vec::with_capacity(wire.tasks.len());
    for (position, task) in wire.tasks.into_iter().enumerate() {
        let expected =
            u32::try_from(position).map_err(|_| ExecutionPlanCodecError::TooManyTasks {
                actual: position.saturating_add(1),
                maximum: EXECUTION_PLAN_V1_MAX_TASKS,
            })?;
        if task.ordinal != expected {
            return Err(ExecutionPlanCodecError::InvalidOrdinal {
                position,
                expected,
                actual: task.ordinal,
            });
        }
        let task_id = parse_task_id(&task.task_id)?;
        if !task_ids.insert(task_id) {
            return Err(ExecutionPlanCodecError::DuplicateTaskId(task_id.value()));
        }
        validate_input(&task.input, position)?;
        let grant_override = task.grant_override.map(validate_wire_grant).transpose()?;
        tasks.push(DurableGroupTaskV1 {
            ordinal: task.ordinal,
            task_id,
            agent_id: task.agent_id,
            input: task.input,
            grant_override,
        });
    }

    validate_edge_count(wire.dependency_edges.len())?;
    let mut dependency_edges = Vec::with_capacity(wire.dependency_edges.len());
    let mut edge_set = HashSet::with_capacity(wire.dependency_edges.len());
    for edge in wire.dependency_edges {
        let blocked_by = parse_task_id(&edge.blocked_by)?;
        let task_id = parse_task_id(&edge.task_id)?;
        validate_edge(blocked_by, task_id, &task_ids, &mut edge_set)?;
        dependency_edges.push(DurableDependencyEdgeV1 {
            blocked_by,
            task_id,
        });
    }
    let mut sorted_edges = dependency_edges.clone();
    sorted_edges.sort_unstable_by_key(|edge| (edge.blocked_by.value(), edge.task_id.value()));
    if dependency_edges != sorted_edges {
        return Err(ExecutionPlanCodecError::NonCanonicalBytes);
    }
    validate_acyclic(&task_ids, &dependency_edges)?;

    Ok(DurableExecutionPlanV1 {
        mode: wire.mode,
        tasks,
        dependency_edges,
    })
}

fn validate_task_count(count: usize, mode: ExecutionMode) -> Result<(), ExecutionPlanCodecError> {
    if count == 0 {
        return Err(ExecutionPlanCodecError::EmptyPlan);
    }
    if count > EXECUTION_PLAN_V1_MAX_TASKS {
        return Err(ExecutionPlanCodecError::TooManyTasks {
            actual: count,
            maximum: EXECUTION_PLAN_V1_MAX_TASKS,
        });
    }
    if mode == ExecutionMode::Consensus && count < 2 {
        return Err(ExecutionPlanCodecError::ConsensusRequiresTwoTasks);
    }
    Ok(())
}

fn validate_edge_count(count: usize) -> Result<(), ExecutionPlanCodecError> {
    if count > EXECUTION_PLAN_V1_MAX_DEPENDENCY_EDGES {
        return Err(ExecutionPlanCodecError::TooManyDependencyEdges {
            actual: count,
            maximum: EXECUTION_PLAN_V1_MAX_DEPENDENCY_EDGES,
        });
    }
    Ok(())
}

fn validate_edge(
    blocked_by: TaskId,
    task_id: TaskId,
    task_ids: &HashSet<TaskId>,
    edge_set: &mut HashSet<(TaskId, TaskId)>,
) -> Result<(), ExecutionPlanCodecError> {
    if !task_ids.contains(&blocked_by) || !task_ids.contains(&task_id) {
        return Err(ExecutionPlanCodecError::DanglingDependency {
            blocked_by: blocked_by.value(),
            task_id: task_id.value(),
        });
    }
    if blocked_by == task_id {
        return Err(ExecutionPlanCodecError::SelfDependency(blocked_by.value()));
    }
    if !edge_set.insert((blocked_by, task_id)) {
        return Err(ExecutionPlanCodecError::DuplicateDependency {
            blocked_by: blocked_by.value(),
            task_id: task_id.value(),
        });
    }
    Ok(())
}

fn validate_acyclic(
    task_ids: &HashSet<TaskId>,
    edges: &[DurableDependencyEdgeV1],
) -> Result<(), ExecutionPlanCodecError> {
    let mut indegree = task_ids
        .iter()
        .copied()
        .map(|id| (id, 0_usize))
        .collect::<HashMap<_, _>>();
    let mut outgoing = HashMap::<TaskId, Vec<TaskId>>::new();
    for edge in edges {
        let Some(degree) = indegree.get_mut(&edge.task_id) else {
            return Err(ExecutionPlanCodecError::DanglingDependency {
                blocked_by: edge.blocked_by.value(),
                task_id: edge.task_id.value(),
            });
        };
        *degree += 1;
        outgoing
            .entry(edge.blocked_by)
            .or_default()
            .push(edge.task_id);
    }
    let mut ready = indegree
        .iter()
        .filter_map(|(id, degree)| (*degree == 0).then_some(*id))
        .collect::<Vec<_>>();
    let mut visited = 0_usize;
    while let Some(id) = ready.pop() {
        visited += 1;
        if let Some(blocked) = outgoing.get(&id) {
            for task_id in blocked {
                let Some(degree) = indegree.get_mut(task_id) else {
                    continue;
                };
                *degree -= 1;
                if *degree == 0 {
                    ready.push(*task_id);
                }
            }
        }
    }
    if visited != task_ids.len() {
        return Err(ExecutionPlanCodecError::DependencyCycle);
    }
    Ok(())
}

fn normalize_grant(grant: &GrantSpec) -> Result<DurableGrantSpecV1, ExecutionPlanCodecError> {
    Ok(DurableGrantSpecV1 {
        capabilities: normalize_set(&grant.capabilities, "capabilities")?,
        max_budget: grant.max_budget.map(format_budget).transpose()?,
        allowed_pallets: normalize_set(&grant.allowed_pallets, "allowed_pallets")?,
    })
}

fn validate_wire_grant(grant: WireGrant) -> Result<DurableGrantSpecV1, ExecutionPlanCodecError> {
    let capabilities = normalize_set(&grant.capabilities, "capabilities")?;
    if capabilities != grant.capabilities {
        return Err(ExecutionPlanCodecError::NonCanonicalGrantSet {
            field: "capabilities",
        });
    }
    let allowed_pallets = normalize_set(&grant.allowed_pallets, "allowed_pallets")?;
    if allowed_pallets != grant.allowed_pallets {
        return Err(ExecutionPlanCodecError::NonCanonicalGrantSet {
            field: "allowed_pallets",
        });
    }
    let max_budget = grant
        .max_budget
        .map(|text| {
            let parsed = text
                .parse::<f64>()
                .map_err(|_| ExecutionPlanCodecError::InvalidGrantBudget)?;
            let canonical = format_budget(parsed)?;
            if text != canonical {
                return Err(ExecutionPlanCodecError::InvalidGrantBudget);
            }
            Ok(text)
        })
        .transpose()?;
    Ok(DurableGrantSpecV1 {
        capabilities,
        max_budget,
        allowed_pallets,
    })
}

fn normalize_set(
    values: &[String],
    field: &'static str,
) -> Result<Vec<String>, ExecutionPlanCodecError> {
    if values.iter().any(String::is_empty) {
        return Err(ExecutionPlanCodecError::EmptyGrantEntry { field });
    }
    let mut normalized = values.to_vec();
    normalized.sort_unstable_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
    normalized.dedup();
    Ok(normalized)
}

fn format_budget(value: f64) -> Result<String, ExecutionPlanCodecError> {
    if !value.is_finite() || value.is_sign_negative() && value != 0.0 {
        return Err(ExecutionPlanCodecError::InvalidGrantBudget);
    }
    if value == 0.0 {
        return Ok("0".to_string());
    }
    let mut buffer = ryu::Buffer::new();
    Ok(buffer.format_finite(value).to_string())
}

fn grant_to_domain(grant: &DurableGrantSpecV1) -> Result<GrantSpec, ExecutionPlanCodecError> {
    let max_budget = grant
        .max_budget
        .as_ref()
        .map(|text| {
            text.parse::<f64>()
                .map_err(|_| ExecutionPlanCodecError::InvalidGrantBudget)
        })
        .transpose()?;
    Ok(GrantSpec {
        capabilities: grant.capabilities.clone(),
        max_budget,
        allowed_pallets: grant.allowed_pallets.clone(),
    })
}

fn validate_input(input: &Value, ordinal: usize) -> Result<(), ExecutionPlanCodecError> {
    validate_input_node(input, 0, ordinal)?;
    let byte_len = canonical_json_len(input).map_err(|error| match error {
        CanonicalWriteError::NonInteger => ExecutionPlanCodecError::NonIntegerTaskInput { ordinal },
    })?;
    if byte_len > EXECUTION_PLAN_V1_MAX_TASK_INPUT_BYTES {
        return Err(ExecutionPlanCodecError::TaskInputTooLarge {
            ordinal,
            actual: byte_len,
            maximum: EXECUTION_PLAN_V1_MAX_TASK_INPUT_BYTES,
        });
    }
    Ok(())
}

fn validate_input_node(
    value: &Value,
    depth: usize,
    ordinal: usize,
) -> Result<(), ExecutionPlanCodecError> {
    if depth > EXECUTION_PLAN_V1_MAX_TASK_INPUT_DEPTH {
        return Err(ExecutionPlanCodecError::TaskInputTooDeep {
            ordinal,
            maximum: EXECUTION_PLAN_V1_MAX_TASK_INPUT_DEPTH,
        });
    }
    match value {
        Value::Array(values) => {
            for value in values {
                validate_input_node(value, depth + 1, ordinal)?;
            }
        }
        Value::Object(values) => {
            for value in values.values() {
                validate_input_node(value, depth + 1, ordinal)?;
            }
        }
        Value::Number(number) if !number.is_i64() && !number.is_u64() => {
            return Err(ExecutionPlanCodecError::NonIntegerTaskInput { ordinal });
        }
        _ => {}
    }
    Ok(())
}

fn parse_task_id(text: &str) -> Result<TaskId, ExecutionPlanCodecError> {
    if text.is_empty()
        || text.bytes().any(|byte| !byte.is_ascii_digit())
        || text.len() > 1 && text.starts_with('0')
    {
        return Err(ExecutionPlanCodecError::InvalidTaskId);
    }
    text.parse::<u64>()
        .map(TaskId::new)
        .map_err(|_| ExecutionPlanCodecError::InvalidTaskId)
}

fn freeze(
    plan: DurableExecutionPlanV1,
) -> Result<CanonicalExecutionPlanV1, ExecutionPlanCodecError> {
    let value = plan_to_value(&plan);
    let byte_len =
        canonical_json_len(&value).map_err(|_| ExecutionPlanCodecError::InvalidStructure)?;
    if byte_len > EXECUTION_PLAN_V1_MAX_CANONICAL_BYTES {
        return Err(ExecutionPlanCodecError::PlanTooLarge {
            actual: byte_len,
            maximum: EXECUTION_PLAN_V1_MAX_CANONICAL_BYTES,
        });
    }
    let mut canonical_bytes = Vec::with_capacity(byte_len);
    write_canonical_json(&value, &mut canonical_bytes)
        .map_err(|_| ExecutionPlanCodecError::InvalidStructure)?;
    let digest = compute_execution_plan_digest_v1(&canonical_bytes);
    Ok(CanonicalExecutionPlanV1 {
        plan,
        canonical_bytes,
        digest,
    })
}

fn plan_to_value(plan: &DurableExecutionPlanV1) -> Value {
    let tasks = plan
        .tasks
        .iter()
        .map(|task| {
            let grant_override = task
                .grant_override
                .as_ref()
                .map_or(Value::Null, grant_to_value);
            Value::Object(Map::from_iter([
                (
                    "ordinal".to_string(),
                    Value::Number(Number::from(task.ordinal)),
                ),
                (
                    "task_id".to_string(),
                    Value::String(task.task_id.value().to_string()),
                ),
                (
                    "agent_id".to_string(),
                    Value::String(task.agent_id.to_string()),
                ),
                ("input".to_string(), task.input.clone()),
                ("grant_override".to_string(), grant_override),
            ]))
        })
        .collect();
    let dependency_edges = plan
        .dependency_edges
        .iter()
        .map(|edge| {
            Value::Object(Map::from_iter([
                (
                    "blocked_by".to_string(),
                    Value::String(edge.blocked_by.value().to_string()),
                ),
                (
                    "task_id".to_string(),
                    Value::String(edge.task_id.value().to_string()),
                ),
            ]))
        })
        .collect();
    Value::Object(Map::from_iter([
        (
            "contract".to_string(),
            Value::String(EXECUTION_PLAN_V1_CONTRACT.to_string()),
        ),
        (
            "schema_version".to_string(),
            Value::Number(Number::from(EXECUTION_PLAN_V1_SCHEMA_VERSION)),
        ),
        (
            "mode".to_string(),
            Value::String(mode_name(plan.mode).to_string()),
        ),
        ("tasks".to_string(), Value::Array(tasks)),
        (
            "dependency_edges".to_string(),
            Value::Array(dependency_edges),
        ),
    ]))
}

fn grant_to_value(grant: &DurableGrantSpecV1) -> Value {
    Value::Object(Map::from_iter([
        (
            "capabilities".to_string(),
            Value::Array(
                grant
                    .capabilities
                    .iter()
                    .cloned()
                    .map(Value::String)
                    .collect(),
            ),
        ),
        (
            "max_budget".to_string(),
            grant
                .max_budget
                .as_ref()
                .map_or(Value::Null, |value| Value::String(value.clone())),
        ),
        (
            "allowed_pallets".to_string(),
            Value::Array(
                grant
                    .allowed_pallets
                    .iter()
                    .cloned()
                    .map(Value::String)
                    .collect(),
            ),
        ),
    ]))
}

const fn mode_name(mode: ExecutionMode) -> &'static str {
    match mode {
        ExecutionMode::Sequential => "sequential",
        ExecutionMode::Parallel => "parallel",
        ExecutionMode::Pipeline => "pipeline",
        ExecutionMode::Consensus => "consensus",
    }
}

#[derive(Debug, Clone, Copy)]
enum CanonicalWriteError {
    NonInteger,
}

fn canonical_json_len(value: &Value) -> Result<usize, CanonicalWriteError> {
    let length = match value {
        Value::Null | Value::Bool(true) => 4,
        Value::Bool(false) => 5,
        Value::Number(number) => {
            if let Some(value) = number.as_i64() {
                value.to_string().len()
            } else if let Some(value) = number.as_u64() {
                value.to_string().len()
            } else {
                return Err(CanonicalWriteError::NonInteger);
            }
        }
        Value::String(value) => json_string_len(value),
        Value::Array(values) => {
            let mut length = 2_usize.saturating_add(values.len().saturating_sub(1));
            for value in values {
                length = length.saturating_add(canonical_json_len(value)?);
            }
            length
        }
        Value::Object(values) => {
            let mut length = 2_usize.saturating_add(values.len().saturating_sub(1));
            for (key, value) in values {
                length = length
                    .saturating_add(json_string_len(key))
                    .saturating_add(1)
                    .saturating_add(canonical_json_len(value)?);
            }
            length
        }
    };
    Ok(length)
}

fn json_string_len(value: &str) -> usize {
    value.chars().fold(2_usize, |length, character| {
        let encoded = match character {
            '"' | '\\' | '\u{0008}' | '\u{000C}' | '\n' | '\r' | '\t' => 2,
            '\u{0000}'..='\u{001F}' => 6,
            _ => character.len_utf8(),
        };
        length.saturating_add(encoded)
    })
}

fn write_canonical_json(value: &Value, output: &mut Vec<u8>) -> Result<(), CanonicalWriteError> {
    match value {
        Value::Null => output.extend_from_slice(b"null"),
        Value::Bool(true) => output.extend_from_slice(b"true"),
        Value::Bool(false) => output.extend_from_slice(b"false"),
        Value::Number(number) => {
            if let Some(value) = number.as_i64() {
                output.extend_from_slice(value.to_string().as_bytes());
            } else if let Some(value) = number.as_u64() {
                output.extend_from_slice(value.to_string().as_bytes());
            } else {
                return Err(CanonicalWriteError::NonInteger);
            }
        }
        Value::String(value) => write_json_string(value, output),
        Value::Array(values) => {
            output.push(b'[');
            for (index, value) in values.iter().enumerate() {
                if index != 0 {
                    output.push(b',');
                }
                write_canonical_json(value, output)?;
            }
            output.push(b']');
        }
        Value::Object(values) => {
            output.push(b'{');
            let mut entries = values.iter().collect::<Vec<_>>();
            entries.sort_unstable_by(|(left, _), (right, _)| left.as_bytes().cmp(right.as_bytes()));
            for (index, (key, value)) in entries.into_iter().enumerate() {
                if index != 0 {
                    output.push(b',');
                }
                write_json_string(key, output);
                output.push(b':');
                write_canonical_json(value, output)?;
            }
            output.push(b'}');
        }
    }
    Ok(())
}

fn write_json_string(value: &str, output: &mut Vec<u8>) {
    output.push(b'"');
    for character in value.chars() {
        match character {
            '"' => output.extend_from_slice(br#"\""#),
            '\\' => output.extend_from_slice(br"\\"),
            '\u{0008}' => output.extend_from_slice(br"\b"),
            '\u{000C}' => output.extend_from_slice(br"\f"),
            '\n' => output.extend_from_slice(br"\n"),
            '\r' => output.extend_from_slice(br"\r"),
            '\t' => output.extend_from_slice(br"\t"),
            '\u{0000}'..='\u{001F}' => {
                let mut escape = String::with_capacity(6);
                let _ = write!(escape, "\\u{:04x}", u32::from(character));
                output.extend_from_slice(escape.as_bytes());
            }
            _ => {
                let mut encoded = [0_u8; 4];
                output.extend_from_slice(character.encode_utf8(&mut encoded).as_bytes());
            }
        }
    }
    output.push(b'"');
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WirePlan {
    contract: String,
    schema_version: u64,
    mode: ExecutionMode,
    tasks: Vec<WireTask>,
    dependency_edges: Vec<WireEdge>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireTask {
    ordinal: u32,
    task_id: String,
    agent_id: AgentId,
    input: Value,
    grant_override: Option<WireGrant>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireGrant {
    capabilities: Vec<String>,
    max_budget: Option<String>,
    allowed_pallets: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireEdge {
    blocked_by: String,
    task_id: String,
}

struct DuplicateSafeValue(Value);

impl<'de> Deserialize<'de> for DuplicateSafeValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(DuplicateSafeVisitor).map(Self)
    }
}

struct DuplicateSafeVisitor;

impl<'de> Visitor<'de> for DuplicateSafeVisitor {
    type Value = Value;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a JSON value without duplicate object keys")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
        Ok(Value::Bool(value))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
        Ok(Value::Number(Number::from(value)))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
        Ok(Value::Number(Number::from(value)))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Number::from_f64(value)
            .map(Value::Number)
            .ok_or_else(|| E::custom("non-finite JSON number"))
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        self.visit_string(value.to_string())
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
        Ok(Value::String(value))
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(Value::Null)
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(Value::Null)
    }

    fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        DuplicateSafeValue::deserialize(deserializer).map(|value| value.0)
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element::<DuplicateSafeValue>()? {
            values.push(value.0);
        }
        Ok(Value::Array(values))
    }

    fn visit_map<A>(self, mut object: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut values = Map::new();
        while let Some(key) = object.next_key::<String>()? {
            if values.contains_key(&key) {
                return Err(de::Error::custom(DUPLICATE_KEY_SENTINEL));
            }
            let value = object.next_value::<DuplicateSafeValue>()?;
            values.insert(key, value.0);
        }
        Ok(Value::Object(values))
    }
}

fn decode_without_duplicate_keys(bytes: &[u8]) -> Result<Value, ExecutionPlanCodecError> {
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let value = DuplicateSafeValue::deserialize(&mut deserializer)
        .map_err(|error| safe_json_error(&error))?;
    deserializer
        .end()
        .map_err(|error| safe_json_error(&error))?;
    Ok(value.0)
}

const DUPLICATE_KEY_SENTINEL: &str = "duplicate-object-key";

fn safe_json_error(error: &serde_json::Error) -> ExecutionPlanCodecError {
    if error.to_string().contains(DUPLICATE_KEY_SENTINEL) {
        ExecutionPlanCodecError::DuplicateJsonKey
    } else {
        ExecutionPlanCodecError::InvalidJson {
            line: error.line(),
            column: error.column(),
        }
    }
}
