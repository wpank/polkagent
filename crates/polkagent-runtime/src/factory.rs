//! Durable runtime factory and handle.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use polkagent_config::{Config, DatabaseBackend};
use polkagent_event::{EventBus, EventReceiver, EventRecorder};
use polkagent_service::{AppService, TimeoutConfig};
use polkagent_store_sqlite::{migrations, SqlitePool, SqliteRunStore};

use crate::adapters::{agent, chain, harness, provider};
use crate::{
    ComponentReadiness, ComponentState, ConfigSource, DurableInteractionService, ReadinessWarning,
    RuntimeError, RuntimeOptions, RuntimeReadiness, WarningCode,
};

/// A process-wide application runtime shared by executable surfaces.
///
/// Cloning this handle retains the same service, pool, event bus, and
/// readiness snapshot. It does not build another runtime.
#[derive(Clone)]
pub struct PolkagentRuntime {
    app: Arc<AppService>,
    pool: SqlitePool,
    event_bus: EventBus,
    interactions: Arc<DurableInteractionService>,
    readiness: Arc<RuntimeReadiness>,
    workdir: Arc<PathBuf>,
}

impl std::fmt::Debug for PolkagentRuntime {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PolkagentRuntime")
            .field("workdir", &self.workdir)
            .field("database_path", &self.readiness.database_path)
            .field("operational", &self.readiness.operational)
            .field("degraded", &self.readiness.is_degraded())
            .finish_non_exhaustive()
    }
}

impl PolkagentRuntime {
    /// Return the shared application service.
    pub fn app(&self) -> &Arc<AppService> {
        &self.app
    }

    /// Return the durable `SQLite` pool.
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Return the shared in-process event bus.
    pub fn event_bus(&self) -> &EventBus {
        &self.event_bus
    }

    /// Subscribe to events published after this call.
    pub fn subscribe_events(&self) -> EventReceiver {
        self.event_bus.subscribe()
    }

    /// Return the durable headless interaction service shared by all surfaces.
    pub fn interactions(&self) -> &Arc<DurableInteractionService> {
        &self.interactions
    }

    /// Return the resolved live service configuration.
    pub fn config(&self) -> Arc<Config> {
        self.app.config()
    }

    /// Return the immutable startup readiness snapshot.
    pub fn readiness(&self) -> &RuntimeReadiness {
        &self.readiness
    }

    /// Return the resolved workspace directory.
    pub fn workdir(&self) -> &Path {
        self.workdir.as_path()
    }
}

/// Builds the concrete stores and adapters used by a shared runtime.
#[derive(Debug, Default, Clone, Copy)]
pub struct RuntimeFactory;

impl RuntimeFactory {
    /// Construct, recover, and rehydrate a runtime.
    ///
    /// Diagnostics are returned in [`RuntimeReadiness`] or emitted through
    /// `tracing`; this method never writes to stdout, preserving ACP stdio
    /// protocol safety.
    #[allow(
        clippy::too_many_lines,
        reason = "startup ordering is kept contiguous so recovery and readiness cannot drift from composition"
    )]
    pub async fn build(options: RuntimeOptions) -> Result<PolkagentRuntime, RuntimeError> {
        let workdir = resolve_workdir(&options.workdir)?;
        let (mut config, config_source) = load_config(&options, &workdir)?;
        if let Some(path) = &options.database_path {
            config.database.sqlite.path = path.to_string_lossy().into_owned();
        }
        if options.read_only {
            config.api.read_only = true;
        }
        if config.database.backend != DatabaseBackend::Sqlite {
            return Err(RuntimeError::UnsupportedDatabase {
                backend: format!("{:?}", config.database.backend).to_lowercase(),
            });
        }
        config.skills.directories = config
            .skills
            .directories
            .iter()
            .map(|path| resolve_skill_directory(path, &workdir))
            .collect::<Result<Vec<_>, _>>()?;
        validate_config(&config)?;

        let skill_paths = if config.skills.auto_load {
            config.skills.directories.clone()
        } else {
            Vec::new()
        };
        let unavailable_skill_directories = skill_paths
            .iter()
            .filter(|path| std::fs::read_dir(path).is_err())
            .count();

        let database_path = resolve_database_path(&config.database.sqlite.path, &workdir)?;
        if database_path == Path::new(":memory:") {
            return Err(RuntimeError::InMemoryDatabase);
        }
        let pool = open_and_migrate(&database_path, config.database.sqlite.checkpoint_on_startup)?;

        let recovered_runs = polkagent_service::lifecycle::recover_stuck_runs(&pool)
            .await
            .map_err(|error| RuntimeError::Recovery {
                message: error.to_string(),
            })?;

        let provider = provider::build(&config, &options)?;
        let harness = harness::build(&config, &options, &workdir)?;
        if provider.executor.is_none() && harness.harness.is_none() {
            return Err(RuntimeError::NoExecutionBackend);
        }
        let chain = chain::build(&options);

        let event_bus = EventBus::with_default_capacity();
        let recorder = EventRecorder::new(Arc::new(pool.clone()), event_bus.clone());
        let shared_pool = Arc::new(pool.clone());
        let mut builder = AppService::builder()
            .with_config(config.clone())
            .with_run_store(shared_pool.clone())
            .with_effect_store(shared_pool.clone())
            .with_event_bus(event_bus.clone())
            .with_event_recorder(recorder)
            .with_provider_registry(provider.registry)
            .with_conversation_store(shared_pool.clone())
            .with_payment_store(shared_pool);

        if !skill_paths.is_empty() {
            builder = builder.with_discovered_skills(skill_paths.clone());
        }

        if let Some(executor) = provider.executor {
            builder = builder.with_executor(executor);
        }
        if let Some(external_harness) = harness.harness {
            builder = builder.with_harness(external_harness);
        }

        let (chain_readiness, tools_readiness) = if let Some(client) = chain.client {
            let mut tool_registry = polkagent_tool::ToolRegistry::new();
            polkagent_tool_governance::register_governance_tools(
                &mut tool_registry,
                Arc::clone(&client),
            );
            polkagent_tool_treasury::register_treasury_tools(
                &mut tool_registry,
                Arc::clone(&client),
            );
            let tool_state = if chain.readiness.state == ComponentState::Degraded {
                ComponentReadiness::degraded(
                    "governance and treasury tools use a simulated chain client",
                )
            } else {
                ComponentReadiness::ready("governance and treasury tools registered")
            };
            builder = builder
                .with_chain_client(client)
                .with_tool_registry(Arc::new(tool_registry));
            (chain.readiness, tool_state)
        } else {
            (
                chain.readiness,
                ComponentReadiness::disabled("chain-backed tools not registered"),
            )
        };

        let (memory_readiness, memory_warning) =
            if config.memory.enabled {
                if config.memory.backend == "sqlite" {
                    let memory = polkagent_memory::SqliteMemoryStore::open(&database_path)
                        .map_err(|error| RuntimeError::Memory {
                            message: error.to_string(),
                        })?;
                    builder = builder.with_memory_store(Arc::new(memory));
                    (
                        ComponentReadiness::ready("SQLite memory store initialized"),
                        None,
                    )
                } else {
                    (
                        ComponentReadiness::unavailable(format!(
                            "memory backend '{}' is not composed",
                            config.memory.backend
                        )),
                        Some(ReadinessWarning::new(
                            WarningCode::MemoryUnavailable,
                            format!(
                                "memory backend '{}' is configured but has no runtime adapter",
                                config.memory.backend
                            ),
                        )),
                    )
                }
            } else {
                (
                    ComponentReadiness::disabled("memory disabled by configuration"),
                    None,
                )
            };

        let service = Arc::new(builder.build().map_err(|error| RuntimeError::Service {
            message: error.to_string(),
        })?);
        let (rehydrated_agents, normalized_legacy_agents) =
            rehydrate_agents(&service, &pool, options.model_override.as_deref())?;

        let interactions = Arc::new(DurableInteractionService::new(
            Arc::clone(&service),
            pool.clone(),
        ));
        let recovered_interaction_turns =
            interactions
                .recover()
                .await
                .map_err(|error| RuntimeError::Recovery {
                    message: format!("durable interaction recovery failed: {error}"),
                })?;

        let timeout_secs = service.config().execution.default_timeout_secs;
        service.start_timeout_enforcer(
            TimeoutConfig::with_global_max(Duration::from_secs(timeout_secs)),
            Duration::from_secs(30),
        );

        let mut warnings = provider.warnings;
        warnings.extend(harness.warnings);
        warnings.extend(chain.warnings);
        if let Some(warning) = memory_warning {
            warnings.push(warning);
        }
        add_sqlite_option_warnings(&config, &mut warnings);
        if normalized_legacy_agents > 0 {
            warnings.push(ReadinessWarning::new(
                WarningCode::LegacyAgentNormalized,
                format!(
                    "normalized {normalized_legacy_agents} legacy agent specification(s) whose ID was stored only in the relational row"
                ),
            ));
        }
        if options.read_only {
            warnings.push(ReadinessWarning::new(
                WarningCode::ReadOnlySurfaceOnly,
                "read-only is represented in API configuration but direct AppService mutations are not gated",
            ));
        }
        warnings.push(ReadinessWarning::new(
            WarningCode::PolicyCompositionIncomplete,
            "AppService currently constructs its internal default policy/grant resolver; configured policy files are not loaded by the public builder",
        ));
        warnings.push(ReadinessWarning::new(
            WarningCode::ShutdownIncomplete,
            "AppService does not yet expose graceful shutdown for its timeout-enforcer task",
        ));

        let operational =
            provider.readiness.state.is_operational() || harness.readiness.state.is_operational();
        let loaded_skill_count = service.skill_manifests().len();
        let skills_readiness = if config.skills.directories.is_empty() {
            ComponentReadiness::disabled("no skill directories configured")
        } else if !config.skills.auto_load {
            ComponentReadiness::disabled(
                "skill directories configured but automatic loading is disabled",
            )
        } else if unavailable_skill_directories > 0 {
            ComponentReadiness::degraded(format!(
                "loaded {loaded_skill_count} skill(s); {unavailable_skill_directories} configured directory path(s) are unavailable"
            ))
        } else {
            ComponentReadiness::ready(format!(
                "loaded {loaded_skill_count} validated skill(s) from {} configured directory path(s)",
                skill_paths.len()
            ))
        };
        let readiness = RuntimeReadiness {
            operational,
            config_source,
            database_path: database_path.clone(),
            database: ComponentReadiness::ready("SQLite opened and migrations applied"),
            executor: provider.readiness,
            harness: harness.readiness,
            chain: chain_readiness,
            tools: tools_readiness,
            effects: ComponentReadiness::ready("durable SQLite EffectStore configured"),
            conversations: ComponentReadiness::ready("durable SQLite ConversationStore configured"),
            payments: ComponentReadiness::ready("durable SQLite PaymentStore configured"),
            memory: memory_readiness,
            signer: ComponentReadiness::unavailable(
                "no production signer selection API is available to the runtime factory",
            ),
            skills: skills_readiness,
            policy_and_grants: ComponentReadiness::degraded(
                "default in-memory policy/grant resolver only",
            ),
            read_only_requested: options.read_only,
            recovered_runs,
            rehydrated_agents,
            warnings,
        };

        tracing::info!(
            database = %database_path.display(),
            rehydrated_agents,
            recovered_runs,
            recovered_interaction_turns,
            loaded_skill_count,
            degraded = readiness.is_degraded(),
            "polkagent runtime ready"
        );

        Ok(PolkagentRuntime {
            app: service,
            pool,
            event_bus,
            interactions,
            readiness: Arc::new(readiness),
            workdir: Arc::new(workdir),
        })
    }
}

fn resolve_workdir(path: &Path) -> Result<PathBuf, RuntimeError> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|_| RuntimeError::InvalidWorkdir {
                path: path.to_path_buf(),
            })?
            .join(path)
    };
    let canonical = absolute
        .canonicalize()
        .map_err(|_| RuntimeError::InvalidWorkdir {
            path: absolute.clone(),
        })?;
    if !canonical.is_dir() {
        return Err(RuntimeError::InvalidWorkdir { path: canonical });
    }
    Ok(canonical)
}

fn load_config(
    options: &RuntimeOptions,
    workdir: &Path,
) -> Result<(Config, ConfigSource), RuntimeError> {
    if let Some(selected) = &options.config_path {
        let path = resolve_under_workdir(selected, workdir);
        let config = polkagent_config::ConfigLoader::new()
            .with_path(&path)
            .load()
            .map_err(|error| RuntimeError::ConfigLoad {
                path: path.clone(),
                message: error.to_string(),
            })?;
        return Ok((config, ConfigSource::Explicit { path }));
    }

    let mut config = Config::default();
    let mut files = Vec::new();
    if let Some(global) = polkagent_config::loader::global_config_path() {
        if global.exists() {
            merge_file(&mut config, &global)?;
            files.push(global);
        }
    }
    if let Some(project) = polkagent_config::loader::find_project_config_from(workdir) {
        if !files.contains(&project) {
            merge_file(&mut config, &project)?;
            files.push(project);
        }
    }
    polkagent_config::env::apply_env_overrides(&mut config);

    let source = if files.is_empty() {
        ConfigSource::Defaults
    } else {
        ConfigSource::Discovered { files }
    };
    Ok((config, source))
}

fn merge_file(config: &mut Config, path: &Path) -> Result<(), RuntimeError> {
    let contents = std::fs::read_to_string(path).map_err(|source| RuntimeError::ConfigRead {
        path: path.to_path_buf(),
        source,
    })?;
    *config = polkagent_config::merge(config.clone(), &contents).map_err(|error| {
        RuntimeError::ConfigLoad {
            path: path.to_path_buf(),
            message: error.to_string(),
        }
    })?;
    Ok(())
}

fn validate_config(config: &Config) -> Result<(), RuntimeError> {
    polkagent_config::validate::validate(config).map_err(|errors| RuntimeError::ConfigValidation {
        message: errors
            .into_iter()
            .map(|error| error.to_string())
            .collect::<Vec<_>>()
            .join("; "),
    })
}

fn resolve_database_path(configured: &str, workdir: &Path) -> Result<PathBuf, RuntimeError> {
    if configured == ":memory:" {
        return Ok(PathBuf::from(configured));
    }
    let expanded = if configured == "~" {
        dirs::home_dir().ok_or_else(|| RuntimeError::Database {
            path: PathBuf::from(configured),
            message: "home directory is unavailable for tilde expansion".to_owned(),
        })?
    } else if let Some(rest) = configured.strip_prefix("~/") {
        dirs::home_dir()
            .ok_or_else(|| RuntimeError::Database {
                path: PathBuf::from(configured),
                message: "home directory is unavailable for tilde expansion".to_owned(),
            })?
            .join(rest)
    } else {
        PathBuf::from(configured)
    };
    Ok(resolve_under_workdir(&expanded, workdir))
}

fn resolve_skill_directory(path: &Path, workdir: &Path) -> Result<PathBuf, RuntimeError> {
    let expanded = if path == Path::new("~") {
        dirs::home_dir().ok_or_else(|| RuntimeError::SkillDirectory {
            path: path.to_path_buf(),
            message: "home directory is unavailable for tilde expansion".to_owned(),
        })?
    } else if let Ok(rest) = path.strip_prefix("~") {
        dirs::home_dir()
            .ok_or_else(|| RuntimeError::SkillDirectory {
                path: path.to_path_buf(),
                message: "home directory is unavailable for tilde expansion".to_owned(),
            })?
            .join(rest)
    } else {
        path.to_path_buf()
    };
    Ok(resolve_under_workdir(&expanded, workdir))
}

fn resolve_under_workdir(path: &Path, workdir: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        workdir.join(path)
    }
}

fn open_and_migrate(path: &Path, checkpoint: bool) -> Result<SqlitePool, RuntimeError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| RuntimeError::DatabaseDirectory {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let pool = SqlitePool::open(path).map_err(|error| RuntimeError::Database {
        path: path.to_path_buf(),
        message: error.to_string(),
    })?;
    {
        let writer = pool.writer();
        migrations::migrate(&writer).map_err(|error| RuntimeError::Database {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
        if checkpoint {
            writer
                .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
                .map_err(|error| RuntimeError::Database {
                    path: path.to_path_buf(),
                    message: error.to_string(),
                })?;
        }
    }
    Ok(pool)
}

fn rehydrate_agents(
    service: &AppService,
    pool: &SqlitePool,
    model_override: Option<&str>,
) -> Result<(usize, usize), RuntimeError> {
    let rows = SqliteRunStore::new(pool.clone())
        .list_agents(Some("active"), false)
        .map_err(|error| RuntimeError::AgentStore {
            message: error.to_string(),
        })?;
    let mut normalized = 0;
    for row in &rows {
        let agent = agent::rehydrate(row, model_override)?;
        normalized += usize::from(agent.normalized_legacy_shape);
        service
            .create_agent(agent.spec)
            .map_err(|error| RuntimeError::AgentSpec {
                agent_id: row.id.clone(),
                message: error.to_string(),
            })?;
    }
    Ok((rows.len(), normalized))
}

fn add_sqlite_option_warnings(config: &Config, warnings: &mut Vec<ReadinessWarning>) {
    if !config.database.sqlite.wal_mode {
        warnings.push(ReadinessWarning::new(
            WarningCode::SqliteOptionIgnored,
            "database.sqlite.wal_mode=false is not honored; SqlitePool always enables WAL",
        ));
    }
    if config.database.sqlite.busy_timeout_ms != 5_000 {
        warnings.push(ReadinessWarning::new(
            WarningCode::SqliteOptionIgnored,
            format!(
                "database.sqlite.busy_timeout_ms={} is not honored; SqlitePool currently fixes it at 5000",
                config.database.sqlite.busy_timeout_ms
            ),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_paths_resolve_under_workdir() {
        assert_eq!(
            resolve_under_workdir(Path::new("data/store.db"), Path::new("/workspace")),
            PathBuf::from("/workspace/data/store.db")
        );
    }

    #[test]
    fn absolute_paths_are_preserved() {
        assert_eq!(
            resolve_under_workdir(Path::new("/data/store.db"), Path::new("/workspace")),
            PathBuf::from("/data/store.db")
        );
    }

    #[test]
    fn relative_skill_directories_resolve_under_workdir() {
        assert_eq!(
            resolve_skill_directory(Path::new("skills"), Path::new("/workspace"))
                .expect("resolve skill directory"),
            PathBuf::from("/workspace/skills")
        );
    }
}
