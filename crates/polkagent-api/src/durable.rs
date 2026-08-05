//! Durable API adapters backed by the shared production runtime.
//!
//! [`RuntimeAgentStore`] persists complete [`AgentSpec`] values in the runtime's
//! `agents` table and keeps the live [`AppService`] registry synchronized.
//! [`RuntimeRunManager`] delegates lifecycle transitions to that same service
//! while projecting the durable `SQLite` records into the public API DTOs.
//!
//! Projection is deliberately strict. Corrupt identifiers, timestamps, JSON,
//! or lifecycle strings are returned as internal errors rather than being
//! rewritten to defaults or reported as missing resources.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use polkagent_config::Config;
use polkagent_core::agent::AgentSpec;
use polkagent_core::run::RunState;
use polkagent_core::{AgentId, EffectId, RunId};
use polkagent_runtime::PolkagentRuntime;
use polkagent_service::{AppService, ServiceError};
use polkagent_store_sqlite::SqlitePool;
use polkagent_store_trait::RunStore;
use rusqlite::OptionalExtension;

use crate::dto::TurnSummary;
use crate::run::{ListRunsParams, RunError, RunManagerTrait, RunRecord};
use crate::state::{AgentStore, AgentStoreError, AppState};

/// One HTTP route that the shared runtime cannot currently back.
///
/// These routes remain registered so clients receive a stable, explicit
/// `501 Not Implemented` response instead of a misleading `404` or a
/// process-local placeholder implementation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnavailableRuntimeRoute {
    /// Runtime capability whose API adapter is missing.
    pub dependency: &'static str,
    /// HTTP method accepted by the registered route.
    pub method: &'static str,
    /// Versioned route template.
    pub path: &'static str,
    /// Exact reason the route is not composed.
    pub reason: &'static str,
}

/// Exact API boundary that remains unavailable in runtime-composed servers.
///
/// Skills, tools, and memory exist inside [`AppService`], but their runtime
/// contracts do not implement the query/mutation ports owned by the API
/// crate. The runtime `SQLite` pool's artifact contract is also distinct from
/// the API artifact port. Audit and service-registry persistence are not
/// composed by [`polkagent_runtime::RuntimeFactory`] at all. No in-memory
/// substitutes are installed for these routes.
pub const RUNTIME_UNAVAILABLE_ROUTES: &[UnavailableRuntimeRoute] = &[
    UnavailableRuntimeRoute {
        dependency: "artifacts",
        method: "GET",
        path: "/api/v1alpha1/runs/{id}/artifacts",
        reason: "the runtime artifact store has no API ArtifactStore adapter",
    },
    UnavailableRuntimeRoute {
        dependency: "artifacts",
        method: "GET",
        path: "/api/v1alpha1/artifacts/{id}",
        reason: "the runtime artifact store has no API ArtifactStore adapter",
    },
    UnavailableRuntimeRoute {
        dependency: "artifacts",
        method: "GET",
        path: "/api/v1alpha1/artifacts/{id}/content",
        reason: "the runtime artifact store has no API ArtifactStore adapter",
    },
    UnavailableRuntimeRoute {
        dependency: "artifacts",
        method: "GET",
        path: "/api/v1alpha1/artifacts/{id}/provenance",
        reason: "the runtime artifact store has no API ArtifactStore adapter",
    },
    UnavailableRuntimeRoute {
        dependency: "skills",
        method: "GET",
        path: "/api/v1alpha1/skills",
        reason: "the runtime skill runner has no API SkillRegistry adapter",
    },
    UnavailableRuntimeRoute {
        dependency: "skills",
        method: "POST",
        path: "/api/v1alpha1/skills/install",
        reason: "the runtime skill runner has no API SkillRegistry adapter",
    },
    UnavailableRuntimeRoute {
        dependency: "skills",
        method: "GET",
        path: "/api/v1alpha1/skills/{skill_id}",
        reason: "the runtime skill runner has no API SkillRegistry adapter",
    },
    UnavailableRuntimeRoute {
        dependency: "skills",
        method: "POST",
        path: "/api/v1alpha1/skills/{skill_id}/uninstall",
        reason: "the runtime skill runner has no API SkillRegistry adapter",
    },
    UnavailableRuntimeRoute {
        dependency: "skills",
        method: "PUT",
        path: "/api/v1alpha1/skills/{skill_id}/config",
        reason: "the runtime skill runner has no API SkillRegistry adapter",
    },
    UnavailableRuntimeRoute {
        dependency: "tools",
        method: "GET",
        path: "/api/v1alpha1/tools",
        reason: "the runtime tool registry has no API ToolRegistryStore adapter",
    },
    UnavailableRuntimeRoute {
        dependency: "tools",
        method: "GET",
        path: "/api/v1alpha1/tools/{tool_id}",
        reason: "the runtime tool registry has no API ToolRegistryStore adapter",
    },
    UnavailableRuntimeRoute {
        dependency: "tools",
        method: "GET",
        path: "/api/v1alpha1/tools/{tool_id}/grants",
        reason: "the runtime tool registry has no API ToolRegistryStore adapter",
    },
    UnavailableRuntimeRoute {
        dependency: "memory",
        method: "POST",
        path: "/api/v1alpha1/memory/query",
        reason: "the runtime memory store has no API MemoryStore adapter",
    },
    UnavailableRuntimeRoute {
        dependency: "memory",
        method: "GET",
        path: "/api/v1alpha1/memory/stats",
        reason: "the runtime memory store has no API MemoryStore adapter",
    },
    UnavailableRuntimeRoute {
        dependency: "memory",
        method: "POST",
        path: "/api/v1alpha1/memory/forget",
        reason: "the runtime memory store has no API MemoryStore adapter",
    },
    UnavailableRuntimeRoute {
        dependency: "memory",
        method: "GET",
        path: "/api/v1alpha1/memory/entries/{entry_id}",
        reason: "the runtime memory store has no API MemoryStore adapter",
    },
    UnavailableRuntimeRoute {
        dependency: "audit",
        method: "GET",
        path: "/api/v1alpha1/audit",
        reason: "RuntimeFactory does not compose an AuditStore",
    },
    UnavailableRuntimeRoute {
        dependency: "audit",
        method: "GET",
        path: "/api/v1alpha1/audit/verify",
        reason: "RuntimeFactory does not compose an AuditStore",
    },
    UnavailableRuntimeRoute {
        dependency: "audit",
        method: "GET",
        path: "/api/v1alpha1/audit/{id}",
        reason: "RuntimeFactory does not compose an AuditStore",
    },
    UnavailableRuntimeRoute {
        dependency: "service_registry",
        method: "POST",
        path: "/api/v1alpha1/registry/listings",
        reason: "RuntimeFactory does not compose a ServiceRegistryStore",
    },
    UnavailableRuntimeRoute {
        dependency: "service_registry",
        method: "GET",
        path: "/api/v1alpha1/registry/listings/{id}",
        reason: "RuntimeFactory does not compose a ServiceRegistryStore",
    },
    UnavailableRuntimeRoute {
        dependency: "service_registry",
        method: "GET",
        path: "/api/v1alpha1/registry/search",
        reason: "RuntimeFactory does not compose a ServiceRegistryStore",
    },
];

/// Compose production API state over one existing shared runtime.
///
/// The caller supplies a clone of the runtime config after applying
/// surface-only overrides such as CORS. Agents and run lifecycle operations
/// use the runtime's [`AppService`]; effects, events, conversations, and
/// payments all use its single migrated `SQLite` pool; WebSocket streaming
/// uses the runtime event bus.
///
/// Optional stores without a truthful adapter are deliberately left unset;
/// their exact `501` boundary is published in
/// [`RUNTIME_UNAVAILABLE_ROUTES`].
#[must_use]
pub fn app_state_from_runtime(runtime: &PolkagentRuntime, config: Config) -> AppState {
    let pool = Arc::new(runtime.pool().clone());
    let agents = Arc::new(RuntimeAgentStore::from_runtime(runtime));
    let runs = Arc::new(RuntimeRunManager::from_runtime(runtime));

    AppState::new(
        config,
        agents,
        runs,
        pool.clone(),
        runtime.event_bus().clone(),
    )
    .with_event_store(pool.clone())
    .with_payment_store(pool.clone())
    .with_conversation_store(pool)
}

/// Durable agent projection coupled to the live application service.
#[derive(Clone)]
pub struct RuntimeAgentStore {
    pool: SqlitePool,
    app: Arc<AppService>,
}

impl std::fmt::Debug for RuntimeAgentStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RuntimeAgentStore")
            .field("database_path", &self.pool.path())
            .finish_non_exhaustive()
    }
}

impl RuntimeAgentStore {
    /// Build an adapter from explicit runtime components.
    #[must_use]
    pub fn new(pool: SqlitePool, app: Arc<AppService>) -> Self {
        Self { pool, app }
    }

    /// Build an adapter over an existing shared runtime handle.
    #[must_use]
    pub fn from_runtime(runtime: &PolkagentRuntime) -> Self {
        Self::new(runtime.pool().clone(), Arc::clone(runtime.app()))
    }
}

#[async_trait]
impl AgentStore for RuntimeAgentStore {
    async fn insert(&self, spec: AgentSpec) -> Result<(), AgentStoreError> {
        spec.validate()
            .map_err(|error| AgentStoreError::InvalidInput(error.to_string()))?;
        let spec_json = serde_json::to_string(&spec)
            .map_err(|error| AgentStoreError::InvalidInput(error.to_string()))?;
        let stored_spec = spec.clone();
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();
            writer.execute(
                "INSERT INTO agents
                    (id, name, description, state, spec_json, created_at, updated_at)
                 VALUES (?1, ?2, ?3, 'active', ?4, ?5, ?6)
                 ON CONFLICT(id) DO UPDATE SET
                    name = excluded.name,
                    description = excluded.description,
                    state = 'active',
                    spec_json = excluded.spec_json,
                    created_at = excluded.created_at,
                    updated_at = excluded.updated_at",
                rusqlite::params![
                    stored_spec.id.to_string(),
                    stored_spec.name,
                    stored_spec.description,
                    spec_json,
                    stored_spec.created_at.to_rfc3339(),
                    stored_spec.updated_at.to_rfc3339(),
                ],
            )
        })
        .await
        .map_err(|error| AgentStoreError::Internal(format!("agent write task failed: {error}")))?
        .map_err(map_agent_write_error)?;

        self.app
            .create_agent(spec)
            .map_err(|error| AgentStoreError::Internal(error.to_string()))?;
        Ok(())
    }

    async fn get(&self, id: AgentId) -> Result<Option<AgentSpec>, AgentStoreError> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || load_agent(&pool, id))
            .await
            .map_err(|error| {
                AgentStoreError::Internal(format!("agent read task failed: {error}"))
            })?
    }

    async fn remove(&self, id: AgentId) -> Result<bool, AgentStoreError> {
        let pool = self.pool.clone();
        let id_text = id.to_string();
        let removed = tokio::task::spawn_blocking(move || {
            let now = Utc::now().to_rfc3339();
            pool.writer()
                .execute(
                    "UPDATE agents SET state = 'archived', updated_at = ?1
                     WHERE id = ?2 AND state != 'archived'",
                    rusqlite::params![now, id_text],
                )
                .map(|changed| changed > 0)
                .map_err(|error| AgentStoreError::Internal(error.to_string()))
        })
        .await
        .map_err(|error| {
            AgentStoreError::Internal(format!("agent archive task failed: {error}"))
        })??;

        if removed {
            self.app
                .remove_agent(id)
                .map_err(|error| AgentStoreError::Internal(error.to_string()))?;
        }
        Ok(removed)
    }

    async fn list_page(
        &self,
        after: Option<AgentId>,
        limit: usize,
    ) -> Result<(Vec<AgentSpec>, bool), AgentStoreError> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();
            let mut statement = writer
                .prepare(
                    "SELECT id, name, description, spec_json, created_at, updated_at
                     FROM agents
                     WHERE state != 'archived'
                     ORDER BY created_at ASC, id ASC",
                )
                .map_err(|error| AgentStoreError::Internal(error.to_string()))?;
            let rows = statement
                .query_map([], |row| {
                    Ok(StoredAgent {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        description: row.get(2)?,
                        spec_json: row.get(3)?,
                        created_at: row.get(4)?,
                        updated_at: row.get(5)?,
                    })
                })
                .map_err(|error| AgentStoreError::Internal(error.to_string()))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| AgentStoreError::Internal(error.to_string()))?;
            let specs = rows
                .into_iter()
                .map(StoredAgent::into_spec)
                .collect::<Result<Vec<_>, _>>()?;
            Ok(agent_page(specs, after, limit))
        })
        .await
        .map_err(|error| AgentStoreError::Internal(format!("agent list task failed: {error}")))?
    }

    async fn count(&self) -> Result<usize, AgentStoreError> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || {
            let count: i64 = pool
                .writer()
                .query_row(
                    "SELECT COUNT(*) FROM agents WHERE state != 'archived'",
                    [],
                    |row| row.get(0),
                )
                .map_err(|error| AgentStoreError::Internal(error.to_string()))?;
            usize::try_from(count).map_err(|_| {
                AgentStoreError::InvalidProjection(format!("invalid agent count {count}"))
            })
        })
        .await
        .map_err(|error| AgentStoreError::Internal(format!("agent count task failed: {error}")))?
    }
}

/// Durable run projection whose mutations flow through [`AppService`].
#[derive(Clone)]
pub struct RuntimeRunManager {
    pool: SqlitePool,
    app: Arc<AppService>,
}

impl std::fmt::Debug for RuntimeRunManager {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RuntimeRunManager")
            .field("database_path", &self.pool.path())
            .finish_non_exhaustive()
    }
}

impl RuntimeRunManager {
    /// Build an adapter from explicit runtime components.
    #[must_use]
    pub fn new(pool: SqlitePool, app: Arc<AppService>) -> Self {
        Self { pool, app }
    }

    /// Build an adapter over an existing shared runtime handle.
    #[must_use]
    pub fn from_runtime(runtime: &PolkagentRuntime) -> Self {
        Self::new(runtime.pool().clone(), Arc::clone(runtime.app()))
    }

    async fn load(&self, run_id: RunId) -> Result<RunRecord, RunError> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || load_run(&pool, run_id))
            .await
            .map_err(|error| RunError::Internal(format!("run read task failed: {error}")))?
    }

    async fn all_runs(&self) -> Result<Vec<RunRecord>, RunError> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || load_all_runs(&pool))
            .await
            .map_err(|error| RunError::Internal(format!("run list task failed: {error}")))?
    }

    async fn mutate_matching_runs<F, Fut>(
        &self,
        agent_id: AgentId,
        predicate: F,
        mutation: impl Fn(Arc<AppService>, RunId) -> Fut,
    ) -> Result<u32, RunError>
    where
        F: Fn(&RunState) -> bool,
        Fut: std::future::Future<Output = Result<(), ServiceError>>,
    {
        let records = self.all_runs().await?;
        let mut changed = 0u32;
        for record in records
            .into_iter()
            .filter(|record| record.agent_id == agent_id && predicate(&record.state))
        {
            mutation(Arc::clone(&self.app), record.id)
                .await
                .map_err(|error| map_service_error(error, Some(record.id)))?;
            changed = changed
                .checked_add(1)
                .ok_or_else(|| RunError::Internal("run mutation count overflow".to_owned()))?;
        }
        Ok(changed)
    }
}

#[async_trait]
impl RunManagerTrait for RuntimeRunManager {
    async fn create_run(
        &self,
        agent_id: AgentId,
        input: serde_json::Value,
    ) -> Result<RunRecord, RunError> {
        let encoded_input = serde_json::to_string(&input)
            .map_err(|error| RunError::Internal(format!("serializing run input: {error}")))?;
        let prompt = match &input {
            serde_json::Value::String(text) => text.clone(),
            _ => encoded_input.clone(),
        };
        let run_id = self
            .app
            .start_run(agent_id, &prompt)
            .await
            .map_err(|error| map_service_error(error, None))?;

        let pool = self.pool.clone();
        let run_id_text = run_id.to_string();
        let updated = tokio::task::spawn_blocking(move || {
            pool.writer()
                .execute(
                    "UPDATE runs SET params_json = ?1 WHERE id = ?2",
                    rusqlite::params![encoded_input, run_id_text],
                )
                .map_err(|error| RunError::Internal(error.to_string()))
        })
        .await
        .map_err(|error| RunError::Internal(format!("run input write task failed: {error}")))??;
        if updated != 1 {
            let _ = self.app.cancel_run(run_id).await;
            return Err(RunError::Internal(format!(
                "run {run_id} disappeared before its input could be persisted"
            )));
        }

        self.load(run_id).await
    }

    async fn get_run(&self, run_id: RunId) -> Result<RunRecord, RunError> {
        self.load(run_id).await
    }

    async fn cancel_run(&self, run_id: RunId) -> Result<RunRecord, RunError> {
        self.app
            .cancel_run(run_id)
            .await
            .map_err(|error| map_service_error(error, Some(run_id)))?;
        self.load(run_id).await
    }

    async fn resume_run(&self, run_id: RunId) -> Result<RunRecord, RunError> {
        self.app
            .resume_run(run_id)
            .await
            .map_err(|error| map_service_error(error, Some(run_id)))?;
        self.load(run_id).await
    }

    async fn list_runs(&self, params: ListRunsParams) -> Result<(Vec<RunRecord>, bool), RunError> {
        let mut records = self.all_runs().await?;
        records.retain(|record| {
            params
                .filter
                .agent_id
                .is_none_or(|agent_id| record.agent_id == agent_id)
                && params
                    .filter
                    .state
                    .as_ref()
                    .is_none_or(|state| &record.state == state)
        });
        let start = params.after.map_or(0, |cursor| {
            records
                .iter()
                .position(|record| record.id == cursor)
                .map_or(0, |position| position + 1)
        });
        let limit = params.limit.min(100) as usize;
        let mut page = records
            .into_iter()
            .skip(start)
            .take(limit.saturating_add(1))
            .collect::<Vec<_>>();
        let has_more = page.len() > limit;
        page.truncate(limit);
        Ok((page, has_more))
    }

    async fn list_turns(&self, run_id: RunId) -> Result<Vec<TurnSummary>, RunError> {
        self.load(run_id).await?;
        let turns = self
            .pool
            .list_turns(run_id)
            .await
            .map_err(|error| RunError::Internal(error.to_string()))?;
        Ok(turns
            .into_iter()
            .map(|turn| TurnSummary {
                sequence: turn.sequence,
                started_at: turn.started_at,
                completed_at: turn.completed_at,
            })
            .collect())
    }

    async fn stop_agent_runs(&self, agent_id: AgentId) -> Result<u32, RunError> {
        self.mutate_matching_runs(
            agent_id,
            |state| !state.is_terminal(),
            |app, run_id| async move { app.cancel_run(run_id).await },
        )
        .await
    }

    async fn pause_agent_runs(&self, agent_id: AgentId) -> Result<u32, RunError> {
        self.mutate_matching_runs(
            agent_id,
            |state| *state == RunState::Running,
            |app, run_id| async move {
                app.pause_run(run_id, &format!("api-pause-{run_id}"))
                    .await
            },
        )
        .await
    }

    async fn resume_agent_runs(&self, agent_id: AgentId) -> Result<u32, RunError> {
        self.mutate_matching_runs(
            agent_id,
            |state| matches!(state, RunState::AwaitingApproval { .. }),
            |app, run_id| async move { app.resume_run(run_id).await },
        )
        .await
    }
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "used directly as a Result::map_err callback, which transfers ownership"
)]
fn map_agent_write_error(error: rusqlite::Error) -> AgentStoreError {
    if polkagent_store_sqlite::StoreError::is_unique_violation(&error) {
        AgentStoreError::Conflict("agent ID or name already exists".to_owned())
    } else {
        AgentStoreError::Internal(error.to_string())
    }
}

struct StoredAgent {
    id: String,
    name: String,
    description: Option<String>,
    spec_json: String,
    created_at: String,
    updated_at: String,
}

impl StoredAgent {
    fn into_spec(self) -> Result<AgentSpec, AgentStoreError> {
        let relational_id = self.id.parse::<AgentId>().map_err(|error| {
            AgentStoreError::InvalidProjection(format!("invalid agent id '{}': {error}", self.id))
        })?;
        let relational_created_at = parse_agent_timestamp(&self.created_at)?;
        let relational_updated_at = parse_agent_timestamp(&self.updated_at)?;
        let spec = serde_json::from_str::<AgentSpec>(&self.spec_json).map_err(|error| {
            AgentStoreError::InvalidProjection(format!(
                "invalid spec JSON for agent {relational_id}: {error}"
            ))
        })?;
        if spec.id != relational_id
            || spec.name != self.name
            || spec.description != self.description
            || spec.created_at != relational_created_at
            || spec.updated_at != relational_updated_at
        {
            return Err(AgentStoreError::InvalidProjection(format!(
                "relational and JSON fields disagree for agent {relational_id}"
            )));
        }
        spec.validate().map_err(|error| {
            AgentStoreError::InvalidProjection(format!(
                "stored spec for agent {relational_id} is invalid: {error}"
            ))
        })?;
        Ok(spec)
    }
}

fn load_agent(pool: &SqlitePool, id: AgentId) -> Result<Option<AgentSpec>, AgentStoreError> {
    let id_text = id.to_string();
    let stored = pool
        .writer()
        .query_row(
            "SELECT id, name, description, spec_json, created_at, updated_at
             FROM agents WHERE id = ?1 AND state != 'archived'",
            [&id_text],
            |row| {
                Ok(StoredAgent {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    description: row.get(2)?,
                    spec_json: row.get(3)?,
                    created_at: row.get(4)?,
                    updated_at: row.get(5)?,
                })
            },
        )
        .optional()
        .map_err(|error| AgentStoreError::Internal(error.to_string()))?;
    stored.map(StoredAgent::into_spec).transpose()
}

fn parse_agent_timestamp(value: &str) -> Result<DateTime<Utc>, AgentStoreError> {
    DateTime::parse_from_rfc3339(value)
        .map(|timestamp| timestamp.with_timezone(&Utc))
        .map_err(|error| {
            AgentStoreError::InvalidProjection(format!("invalid timestamp '{value}': {error}"))
        })
}

fn agent_page(
    specs: Vec<AgentSpec>,
    after: Option<AgentId>,
    limit: usize,
) -> (Vec<AgentSpec>, bool) {
    let start = after.map_or(0, |cursor| {
        specs
            .iter()
            .position(|spec| spec.id == cursor)
            .map_or(0, |position| position + 1)
    });
    let mut page = specs
        .into_iter()
        .skip(start)
        .take(limit.saturating_add(1))
        .collect::<Vec<_>>();
    let has_more = page.len() > limit;
    page.truncate(limit);
    (page, has_more)
}

struct StoredRun {
    id: String,
    agent_id: String,
    state: String,
    params_json: String,
    created_at: String,
    started_at: Option<String>,
    completed_at: Option<String>,
    turns_completed: i64,
}

impl StoredRun {
    fn into_record(self) -> Result<RunRecord, RunError> {
        let id = self.id.parse::<RunId>().map_err(|error| {
            RunError::Internal(format!("invalid stored run id '{}': {error}", self.id))
        })?;
        let agent_id = self.agent_id.parse::<AgentId>().map_err(|error| {
            RunError::Internal(format!(
                "invalid stored agent id '{}': {error}",
                self.agent_id
            ))
        })?;
        let state = parse_run_state(&self.state)?;
        let input = serde_json::from_str(&self.params_json).map_err(|error| {
            RunError::Internal(format!("invalid input JSON for run {id}: {error}"))
        })?;
        let turns_completed = u32::try_from(self.turns_completed).map_err(|_| {
            RunError::Internal(format!(
                "invalid completed turn count {} for run {id}",
                self.turns_completed
            ))
        })?;
        let created_at = parse_run_timestamp(id, "created_at", &self.created_at)?;
        let started_at = self
            .started_at
            .as_deref()
            .map(|value| parse_run_timestamp(id, "started_at", value))
            .transpose()?;
        let completed_at = self
            .completed_at
            .as_deref()
            .map(|value| parse_run_timestamp(id, "completed_at", value))
            .transpose()?;
        let terminal_reason = match &state {
            RunState::Failed { reason } | RunState::Cancelled { reason } => Some(reason.clone()),
            _ => None,
        };
        Ok(RunRecord {
            id,
            agent_id,
            state,
            input,
            turns_completed,
            created_at,
            started_at,
            completed_at,
            terminal_reason,
        })
    }
}

fn row_to_stored_run(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredRun> {
    Ok(StoredRun {
        id: row.get(0)?,
        agent_id: row.get(1)?,
        state: row.get(2)?,
        params_json: row.get(3)?,
        created_at: row.get(4)?,
        started_at: row.get(5)?,
        completed_at: row.get(6)?,
        turns_completed: row.get(7)?,
    })
}

const RUN_PROJECTION_SQL: &str = "SELECT r.id, r.agent_id, r.state, r.params_json, r.created_at,
            r.started_at, r.completed_at,
            (SELECT COUNT(*) FROM turns t
             WHERE t.run_id = r.id AND t.completed_at IS NOT NULL)
     FROM runs r";

fn load_run(pool: &SqlitePool, run_id: RunId) -> Result<RunRecord, RunError> {
    let run_id_text = run_id.to_string();
    let sql = format!("{RUN_PROJECTION_SQL} WHERE r.id = ?1");
    let stored = pool
        .writer()
        .query_row(&sql, [&run_id_text], row_to_stored_run)
        .map_err(|error| match error {
            rusqlite::Error::QueryReturnedNoRows => RunError::NotFound(run_id),
            other => RunError::Internal(other.to_string()),
        })?;
    stored.into_record()
}

fn load_all_runs(pool: &SqlitePool) -> Result<Vec<RunRecord>, RunError> {
    let sql = format!("{RUN_PROJECTION_SQL} ORDER BY r.created_at ASC, r.id ASC");
    let writer = pool.writer();
    let mut statement = writer
        .prepare(&sql)
        .map_err(|error| RunError::Internal(error.to_string()))?;
    let stored = statement
        .query_map([], row_to_stored_run)
        .map_err(|error| RunError::Internal(error.to_string()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| RunError::Internal(error.to_string()))?;
    stored.into_iter().map(StoredRun::into_record).collect()
}

fn parse_run_timestamp(run_id: RunId, field: &str, value: &str) -> Result<DateTime<Utc>, RunError> {
    DateTime::parse_from_rfc3339(value)
        .map(|timestamp| timestamp.with_timezone(&Utc))
        .map_err(|error| {
            RunError::Internal(format!(
                "invalid {field} timestamp '{value}' for run {run_id}: {error}"
            ))
        })
}

fn parse_run_state(value: &str) -> Result<RunState, RunError> {
    if let Some(reason) = value.strip_prefix("failed:") {
        return Ok(RunState::Failed {
            reason: reason.to_owned(),
        });
    }
    if let Some(reason) = value.strip_prefix("cancelled:") {
        return Ok(RunState::Cancelled {
            reason: reason.to_owned(),
        });
    }
    if let Some(request_id) = value.strip_prefix("awaiting_approval:") {
        return Ok(RunState::AwaitingApproval {
            request_id: request_id.to_owned(),
        });
    }
    if let Some(ids) = value.strip_prefix("waiting_effect:") {
        let pending_intent_ids = if ids.is_empty() {
            Vec::new()
        } else {
            ids.split(',')
                .map(|id| {
                    id.parse::<EffectId>().map_err(|error| {
                        RunError::Internal(format!(
                            "invalid effect id '{id}' in stored state '{value}': {error}"
                        ))
                    })
                })
                .collect::<Result<Vec<_>, _>>()?
        };
        return Ok(RunState::WaitingEffect { pending_intent_ids });
    }

    match value {
        "created" => Ok(RunState::Created),
        "queued" => Ok(RunState::Queued),
        "running" => Ok(RunState::Running),
        "completing" => Ok(RunState::Completing),
        "completed" => Ok(RunState::Completed),
        "timed_out" => Ok(RunState::TimedOut),
        other => Err(RunError::Internal(format!(
            "unknown stored run state '{other}'"
        ))),
    }
}

fn map_service_error(error: ServiceError, run_id: Option<RunId>) -> RunError {
    match error {
        ServiceError::AgentNotFound { agent_id } => RunError::AgentNotFound(agent_id),
        ServiceError::RunNotFound { run_id } => RunError::NotFound(run_id),
        ServiceError::InvalidTransition { message } => match run_id {
            Some(id) => RunError::InvalidTransition {
                id,
                reason: message,
            },
            None => {
                RunError::Internal(format!("run creation hit an invalid transition: {message}"))
            }
        },
        other => RunError::Internal(other.to_string()),
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "durability tests fail immediately at controlled fixture boundaries"
)]
mod tests {
    use std::time::Duration;

    use polkagent_core::{RunId, TurnId};
    use polkagent_runtime::{AdapterPolicy, RuntimeFactory, RuntimeOptions};
    use polkagent_store_sqlite::migrations;
    use polkagent_store_trait::{RunStatus, RunStore};

    use super::*;

    async fn runtime_at(
        root: &std::path::Path,
        database_path: &std::path::Path,
    ) -> PolkagentRuntime {
        let config_path = root.join("polkagent.toml");
        std::fs::write(&config_path, "").expect("write empty config");
        let mut options = RuntimeOptions::new(root);
        options.config_path = Some(config_path);
        options.database_path = Some(database_path.to_path_buf());
        options.disable_harness = true;
        options.adapter_policy = AdapterPolicy::AllowSimulated;
        options.discover_environment_providers = false;
        RuntimeFactory::build(options).await.expect("build runtime")
    }

    fn agent(name: &str) -> AgentSpec {
        AgentSpec::new(AgentId::new(), name, "fake/default-model")
    }

    #[tokio::test]
    async fn agent_projection_roundtrips_pages_archives_and_restarts() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let database_path = temp.path().join("agents.db");
        let runtime = runtime_at(temp.path(), &database_path).await;
        let store = RuntimeAgentStore::from_runtime(&runtime);

        let mut first = agent("first");
        let mut second = agent("second");
        first.description = Some("complete DTO".to_owned());
        first.declared_capabilities = vec!["chain.query".to_owned()];
        second.created_at = first.created_at + chrono::Duration::seconds(1);
        second.updated_at = second.created_at;
        store.insert(second.clone()).await.expect("insert second");
        store.insert(first.clone()).await.expect("insert first");

        assert_eq!(
            store.get(first.id).await.expect("get first"),
            Some(first.clone())
        );
        assert_eq!(store.count().await.expect("agent count"), 2);
        let (page_one, has_more) = store.list_page(None, 1).await.expect("first page");
        assert_eq!(page_one, vec![first.clone()]);
        assert!(has_more);
        let (page_two, has_more) = store
            .list_page(Some(first.id), 1)
            .await
            .expect("second page");
        assert_eq!(page_two, vec![second.clone()]);
        assert!(!has_more);

        assert!(store.remove(first.id).await.expect("archive first"));
        assert!(!store.remove(first.id).await.expect("archive first again"));
        assert!(store.get(first.id).await.expect("get archived").is_none());
        assert!(matches!(
            runtime.app().start_run(first.id, "must fail closed").await,
            Err(ServiceError::AgentNotFound { .. })
        ));

        drop(store);
        drop(runtime);
        let restarted = runtime_at(temp.path(), &database_path).await;
        let restarted_store = RuntimeAgentStore::from_runtime(&restarted);
        assert!(restarted_store
            .get(first.id)
            .await
            .expect("archived after restart")
            .is_none());
        assert_eq!(
            restarted_store
                .get(second.id)
                .await
                .expect("active after restart"),
            Some(second)
        );
    }

    #[tokio::test]
    async fn agent_projection_rejects_name_conflicts_and_corrupt_rows() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let database_path = temp.path().join("agent-errors.db");
        let runtime = runtime_at(temp.path(), &database_path).await;
        let store = RuntimeAgentStore::from_runtime(&runtime);
        let first = agent("duplicate");
        let duplicate = agent("duplicate");
        store.insert(first.clone()).await.expect("insert first");
        assert!(matches!(
            store.insert(duplicate).await,
            Err(AgentStoreError::Conflict(_))
        ));

        runtime
            .pool()
            .writer()
            .execute(
                "UPDATE agents SET spec_json = '{}' WHERE id = ?1",
                [first.id.to_string()],
            )
            .expect("corrupt stored spec");
        assert!(matches!(
            store.get(first.id).await,
            Err(AgentStoreError::InvalidProjection(_))
        ));
    }

    #[tokio::test]
    async fn run_projection_executes_and_preserves_opaque_input_across_restart() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let database_path = temp.path().join("runs.db");
        let runtime = runtime_at(temp.path(), &database_path).await;
        let agents = RuntimeAgentStore::from_runtime(&runtime);
        let runs = RuntimeRunManager::from_runtime(&runtime);
        let spec = agent("runner");
        agents.insert(spec.clone()).await.expect("insert agent");
        let input = serde_json::json!({
            "prompt": "inspect treasury",
            "context": {"network": "polkadot"},
            "attempt": 2,
        });

        let created = runs
            .create_run(spec.id, input.clone())
            .await
            .expect("create runtime run");
        assert_eq!(created.agent_id, spec.id);
        assert_eq!(created.input, input);

        let terminal = tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let record = runs.get_run(created.id).await.expect("reload run");
                if record.state.is_terminal() {
                    break record;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("fake run should terminate");
        assert_eq!(terminal.input, input);
        assert_eq!(terminal.state, RunState::Completed);

        drop(runs);
        drop(agents);
        drop(runtime);
        let restarted = runtime_at(temp.path(), &database_path).await;
        let restarted_runs = RuntimeRunManager::from_runtime(&restarted);
        let reloaded = restarted_runs
            .get_run(created.id)
            .await
            .expect("run after restart");
        assert_eq!(reloaded.input, input);
        assert_eq!(reloaded.state, RunState::Completed);
    }

    #[tokio::test]
    async fn lifecycle_mutations_use_service_state_machine_and_record_events() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let database_path = temp.path().join("lifecycle.db");
        let runtime = runtime_at(temp.path(), &database_path).await;
        let agents = RuntimeAgentStore::from_runtime(&runtime);
        let runs = RuntimeRunManager::from_runtime(&runtime);
        let spec = agent("lifecycle");
        agents.insert(spec.clone()).await.expect("insert agent");

        let run_id = RunId::new();
        RunStore::create(
            runtime.pool(),
            run_id,
            &spec.id.to_string(),
            RunStatus::new("running"),
        )
        .await
        .expect("insert running run");

        assert_eq!(
            runs.pause_agent_runs(spec.id).await.expect("pause agent"),
            1
        );
        assert!(matches!(
            runs.get_run(run_id).await.expect("paused run").state,
            RunState::AwaitingApproval { .. }
        ));
        assert_eq!(
            runs.resume_agent_runs(spec.id).await.expect("resume agent"),
            1
        );
        assert_eq!(
            runs.get_run(run_id).await.expect("resumed run").state,
            RunState::Running
        );
        let cancelled = runs.cancel_run(run_id).await.expect("cancel run");
        assert_eq!(
            cancelled.state,
            RunState::Cancelled {
                reason: "cancelled by user".to_owned()
            }
        );
        assert_eq!(
            cancelled.terminal_reason.as_deref(),
            Some("cancelled by user")
        );

        let event_count: i64 = runtime
            .pool()
            .writer()
            .query_row(
                "SELECT COUNT(*) FROM run_events WHERE run_id = ?1",
                [run_id.to_string()],
                |row| row.get(0),
            )
            .expect("count lifecycle events");
        assert_eq!(event_count, 3);
    }

    #[tokio::test]
    async fn run_lists_and_turns_preserve_filters_cursor_and_completion_semantics() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let database_path = temp.path().join("list.db");
        let runtime = runtime_at(temp.path(), &database_path).await;
        let agents = RuntimeAgentStore::from_runtime(&runtime);
        let runs = RuntimeRunManager::from_runtime(&runtime);
        let first_agent = agent("list-first");
        let second_agent = agent("list-second");
        agents
            .insert(first_agent.clone())
            .await
            .expect("insert first agent");
        agents
            .insert(second_agent.clone())
            .await
            .expect("insert second agent");

        let first_run = RunId::new();
        let second_run = RunId::new();
        let other_run = RunId::new();
        for (run_id, agent_id, state) in [
            (first_run, first_agent.id, "running"),
            (second_run, first_agent.id, "completed"),
            (other_run, second_agent.id, "running"),
        ] {
            RunStore::create(
                runtime.pool(),
                run_id,
                &agent_id.to_string(),
                RunStatus::new(state),
            )
            .await
            .expect("insert projected run");
        }
        let turn_id = TurnId::new();
        let started_at = Utc::now();
        runtime
            .pool()
            .insert_turn(
                turn_id,
                first_run,
                0,
                "assistant",
                &started_at.to_rfc3339(),
                Some(&started_at.to_rfc3339()),
                2,
                3,
            )
            .await
            .expect("insert completed turn");

        let (first_page, has_more) = runs
            .list_runs(ListRunsParams {
                after: None,
                limit: 1,
                filter: crate::run::RunFilter {
                    agent_id: Some(first_agent.id),
                    state: None,
                },
            })
            .await
            .expect("first page");
        assert_eq!(first_page.len(), 1);
        assert!(has_more);
        let (second_page, has_more) = runs
            .list_runs(ListRunsParams {
                after: Some(first_page[0].id),
                limit: 1,
                filter: crate::run::RunFilter {
                    agent_id: Some(first_agent.id),
                    state: None,
                },
            })
            .await
            .expect("second page");
        assert_eq!(second_page.len(), 1);
        assert!(!has_more);
        let (completed, _) = runs
            .list_runs(ListRunsParams {
                after: None,
                limit: 100,
                filter: crate::run::RunFilter {
                    agent_id: None,
                    state: Some(RunState::Completed),
                },
            })
            .await
            .expect("completed filter");
        assert_eq!(completed.len(), 1);
        assert_eq!(completed[0].id, second_run);

        let turns = runs.list_turns(first_run).await.expect("list turns");
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].sequence, 0);
        assert_eq!(
            runs.get_run(first_run)
                .await
                .expect("run with completed turn")
                .turns_completed,
            1
        );
    }

    #[tokio::test]
    async fn corrupt_run_state_fails_closed() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let database_path = temp.path().join("corrupt-run.db");
        let runtime = runtime_at(temp.path(), &database_path).await;
        migrations::migrate(&runtime.pool().writer()).expect("migrations remain idempotent");
        let agents = RuntimeAgentStore::from_runtime(&runtime);
        let runs = RuntimeRunManager::from_runtime(&runtime);
        let spec = agent("corrupt-run-agent");
        agents.insert(spec.clone()).await.expect("insert agent");
        let run_id = RunId::new();
        RunStore::create(
            runtime.pool(),
            run_id,
            &spec.id.to_string(),
            RunStatus::new("created"),
        )
        .await
        .expect("insert run");
        runtime
            .pool()
            .writer()
            .execute(
                "UPDATE runs SET state = 'invented-state' WHERE id = ?1",
                [run_id.to_string()],
            )
            .expect("corrupt run state");
        assert!(matches!(
            runs.get_run(run_id).await,
            Err(RunError::Internal(_))
        ));
    }
}
