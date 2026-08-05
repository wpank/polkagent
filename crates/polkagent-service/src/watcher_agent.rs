//! Always-on watcher agents (PRD-08 §7, deliverable 6.6).
//!
//! [`WatcherAgentManager`] turns the declarative `[[watchers]]` config entries
//! into scheduled background tasks that run on the `polkagent-scheduler`
//! infrastructure. Each watcher:
//!
//! 1. Gets its own Cedar policy grant (read-only by default).
//! 2. Runs on the schedule declared in its [`WatcherConfig`].
//! 3. Has strictly **no write authority** unless a separate Cedar grant is
//!    issued that explicitly permits mutating effects.
//! 4. Is subject to `TimeoutEnforcer` (DX-3) — the manager refuses to start
//!    if timeout enforcement is not active.
//!
//! # Feature gate
//!
//! This module is only compiled when the `watcher` feature is enabled on
//! `polkagent-service`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, Utc};
use polkagent_config::{WatcherConfig, WatcherSchedule};
use polkagent_core::AgentId;
use polkagent_grant::policy::{Effect, PolicyRule, PolicySet};
use polkagent_scheduler::{
    CronExpr, InMemoryTaskStore, Schedule, ScheduledTask, Scheduler, SchedulerError, TaskAction,
    TaskId,
};
use tracing::{info, warn};

use crate::error::ServiceError;

// ---------------------------------------------------------------------------
// WatcherHistoryEntry
// ---------------------------------------------------------------------------

/// A record of a single watcher execution.
#[derive(Debug, Clone)]
pub struct WatcherHistoryEntry {
    /// The scheduler task ID that fired.
    pub task_id: TaskId,
    /// Human-readable watcher name.
    pub watcher_name: String,
    /// The agent that was invoked.
    pub agent_id: AgentId,
    /// Cedar policy governing this execution.
    pub cedar_policy: String,
    /// Whether the watcher was in read-only mode.
    pub read_only: bool,
    /// When the execution was triggered.
    pub fired_at: DateTime<Utc>,
    /// Whether the run was successfully started.
    pub success: bool,
    /// Error message if the run failed to start.
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------
// WatcherAgentManager
// ---------------------------------------------------------------------------

/// Manages always-on watcher agents.
///
/// Each watcher declared in `[[watchers]]` config is registered with the
/// scheduler and runs according to its schedule. The manager enforces that:
///
/// - Each watcher gets its own Cedar `PolicySet` (defaulting to read-only).
/// - `TimeoutEnforcer` is active before any watcher is started.
/// - Write effects are denied unless an explicit grant is issued.
pub struct WatcherAgentManager {
    /// The underlying scheduler.
    scheduler: Scheduler<InMemoryTaskStore, WatcherTaskExecutor>,
    /// Execution history log.
    history: Arc<Mutex<Vec<WatcherHistoryEntry>>>,
    /// Cedar policy sets per watcher name.
    watcher_policies: HashMap<String, PolicySet>,
    /// Whether the `TimeoutEnforcer` is active.
    timeout_enforcer_active: bool,
    /// Registered watcher configs (name → config).
    registered: HashMap<String, WatcherConfig>,
    /// Mapping from watcher name → scheduler task ID.
    task_ids: HashMap<String, TaskId>,
}

impl std::fmt::Debug for WatcherAgentManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WatcherAgentManager")
            .field("registered_count", &self.registered.len())
            .field("timeout_enforcer_active", &self.timeout_enforcer_active)
            .field("history_len", &self.history.lock().map_or(0, |h| h.len()))
            .finish_non_exhaustive()
    }
}

impl WatcherAgentManager {
    /// Create a new manager.
    ///
    /// `timeout_enforcer_active` indicates whether the `TimeoutEnforcer` (DX-3)
    /// is wired up. Watchers cannot be registered unless this is `true`.
    #[must_use]
    pub fn new(poll_interval: Duration, timeout_enforcer_active: bool) -> Self {
        let history: Arc<Mutex<Vec<WatcherHistoryEntry>>> = Arc::new(Mutex::new(Vec::new()));
        let store = Arc::new(InMemoryTaskStore::new());
        let executor = Arc::new(WatcherTaskExecutor {
            history: Arc::clone(&history),
        });
        let scheduler = Scheduler::new(store, executor, poll_interval);

        Self {
            scheduler,
            history,
            watcher_policies: HashMap::new(),
            timeout_enforcer_active,
            registered: HashMap::new(),
            task_ids: HashMap::new(),
        }
    }

    /// Register a watcher from its config entry.
    ///
    /// Creates a Cedar read-only policy set for the watcher and schedules it
    /// with the scheduler.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::Config`] if the `TimeoutEnforcer` is not active,
    /// or [`ServiceError::Internal`] if the scheduler rejects the task.
    pub async fn register_watcher(
        &mut self,
        config: &WatcherConfig,
    ) -> Result<TaskId, ServiceError> {
        if !self.timeout_enforcer_active {
            return Err(ServiceError::Config {
                message: format!(
                    "cannot register watcher '{}': TimeoutEnforcer (DX-3) is not active",
                    config.name
                ),
            });
        }

        if !config.enabled {
            return Err(ServiceError::Config {
                message: format!("watcher '{}' is disabled", config.name),
            });
        }

        // Build Cedar policy set for this watcher.
        let policy_set = build_watcher_policy(&config.cedar_policy, config.read_only);
        self.watcher_policies
            .insert(config.name.clone(), policy_set);

        // Convert WatcherSchedule → scheduler Schedule.
        let schedule = to_scheduler_schedule(&config.schedule)?;

        let agent_id = AgentId::new();
        let task = ScheduledTask::new(
            &config.name,
            schedule,
            agent_id,
            TaskAction::RunAgent {
                agent_id,
                input: serde_json::json!({
                    "watcher": config.name,
                    "cedar_policy": config.cedar_policy,
                    "read_only": config.read_only,
                    "timeout_secs": config.timeout_secs,
                }),
            },
        );
        let task_id = task.id;

        self.scheduler
            .add_task(task)
            .await
            .map_err(|e| ServiceError::Internal {
                message: format!("failed to register watcher '{}': {e}", config.name),
            })?;

        self.registered.insert(config.name.clone(), config.clone());
        self.task_ids.insert(config.name.clone(), task_id);

        info!(
            watcher = %config.name,
            %task_id,
            read_only = config.read_only,
            "watcher agent registered"
        );
        Ok(task_id)
    }

    /// Register all enabled watchers from a config slice.
    ///
    /// Disabled watchers are silently skipped. Returns the number of
    /// watchers successfully registered.
    ///
    /// # Errors
    ///
    /// Returns an error if the `TimeoutEnforcer` is not active.
    pub async fn register_all(&mut self, configs: &[WatcherConfig]) -> Result<usize, ServiceError> {
        let mut count = 0;
        for config in configs {
            if !config.enabled {
                info!(watcher = %config.name, "skipping disabled watcher");
                continue;
            }
            self.register_watcher(config).await?;
            count += 1;
        }
        Ok(count)
    }

    /// Unregister a watcher by name.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::Internal`] if the watcher is not registered or
    /// the scheduler rejects the cancellation.
    pub async fn unregister_watcher(&mut self, name: &str) -> Result<(), ServiceError> {
        let task_id = self
            .task_ids
            .remove(name)
            .ok_or_else(|| ServiceError::Internal {
                message: format!("watcher '{name}' is not registered"),
            })?;

        self.scheduler
            .remove_task(task_id)
            .await
            .map_err(|e| ServiceError::Internal {
                message: format!("failed to unregister watcher '{name}': {e}"),
            })?;

        self.registered.remove(name);
        self.watcher_policies.remove(name);

        info!(watcher = %name, %task_id, "watcher agent unregistered");
        Ok(())
    }

    /// Return the Cedar [`PolicySet`] for a named watcher.
    pub fn watcher_policy(&self, name: &str) -> Option<&PolicySet> {
        self.watcher_policies.get(name)
    }

    /// Return a snapshot of the execution history.
    pub fn history(&self) -> Vec<WatcherHistoryEntry> {
        self.history.lock().map(|h| h.clone()).unwrap_or_default()
    }

    /// Run a single poll cycle.
    ///
    /// Returns the number of watchers that fired.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::Internal`] on poll failures.
    pub async fn poll_once(&self) -> Result<usize, ServiceError> {
        let runner = self.scheduler.runner();
        runner
            .poll_once()
            .await
            .map_err(|e| ServiceError::Internal {
                message: format!("watcher poll failed: {e}"),
            })
    }

    /// Return the list of registered watcher names.
    pub fn registered_watchers(&self) -> Vec<String> {
        self.registered.keys().cloned().collect()
    }

    /// Return true if the `TimeoutEnforcer` is active.
    pub fn timeout_enforcer_active(&self) -> bool {
        self.timeout_enforcer_active
    }

    /// Return a reference to the underlying task store.
    #[must_use]
    pub fn store(&self) -> &Arc<InMemoryTaskStore> {
        self.scheduler.store()
    }
}

// ---------------------------------------------------------------------------
// Cedar policy construction
// ---------------------------------------------------------------------------

/// Build a Cedar `PolicySet` for a watcher.
///
/// If `read_only` is `true`, the policy explicitly denies all write actions
/// (`chain.submit`, `chain.transfer`, `effect.*`). A read-only watcher may
/// still perform `chain.query`, `chain.decode`, and `model.inference`.
fn build_watcher_policy(cedar_policy_name: &str, read_only: bool) -> PolicySet {
    let mut set = PolicySet::default();

    // Allow read actions.
    set.add_rule(PolicyRule {
        id: format!("{cedar_policy_name}/allow-read"),
        effect: Effect::Allow,
        action_patterns: vec![
            "chain.query".to_owned(),
            "chain.decode".to_owned(),
            "model.inference".to_owned(),
        ],
        resource_patterns: vec!["**".to_owned()],
        conditions: HashMap::new(),
        abac_condition: None,
    });

    if read_only {
        // Deny all write actions — deny-overrides means this blocks even
        // if another rule allows.
        set.add_rule(PolicyRule {
            id: format!("{cedar_policy_name}/deny-writes"),
            effect: Effect::Deny,
            action_patterns: vec![
                "chain.submit".to_owned(),
                "chain.transfer".to_owned(),
                "effect.*".to_owned(),
            ],
            resource_patterns: vec!["**".to_owned()],
            conditions: HashMap::new(),
            abac_condition: None,
        });
    }

    set
}

// ---------------------------------------------------------------------------
// Schedule conversion
// ---------------------------------------------------------------------------

/// Convert a [`WatcherSchedule`] into a `polkagent-scheduler` [`Schedule`].
///
/// # Errors
///
/// Returns [`ServiceError::Config`] if a cron expression cannot be parsed.
fn to_scheduler_schedule(ws: &WatcherSchedule) -> Result<Schedule, ServiceError> {
    match ws {
        WatcherSchedule::Interval { every_secs } => {
            let seconds = i64::try_from(*every_secs).map_err(|_| ServiceError::Config {
                message: format!("watcher interval {every_secs} exceeds the supported range"),
            })?;
            Ok(Schedule::Interval {
                every: chrono::Duration::seconds(seconds),
                start: Utc::now(),
            })
        }
        WatcherSchedule::Cron { expr } => {
            let cron = CronExpr::parse(expr).map_err(|e| ServiceError::Config {
                message: format!("invalid cron expression '{expr}': {e}"),
            })?;
            Ok(Schedule::Cron(cron))
        }
    }
}

// ---------------------------------------------------------------------------
// WatcherTaskExecutor
// ---------------------------------------------------------------------------

/// A [`TaskExecutor`] that records watcher execution history.
struct WatcherTaskExecutor {
    history: Arc<Mutex<Vec<WatcherHistoryEntry>>>,
}

impl std::fmt::Debug for WatcherTaskExecutor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WatcherTaskExecutor").finish()
    }
}

#[async_trait::async_trait]
impl polkagent_scheduler::TaskExecutor for WatcherTaskExecutor {
    async fn execute(
        &self,
        task: &ScheduledTask,
    ) -> Result<polkagent_scheduler::TaskResult, SchedulerError> {
        let input = match &task.action {
            TaskAction::RunAgent { input, .. } => input.clone(),
            _ => serde_json::Value::Null,
        };

        let watcher_name = input
            .get("watcher")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_owned();
        let cedar_policy = input
            .get("cedar_policy")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_owned();
        let read_only = input
            .get("read_only")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true);

        let entry = WatcherHistoryEntry {
            task_id: task.id,
            watcher_name: watcher_name.clone(),
            agent_id: task.agent_id,
            cedar_policy,
            read_only,
            fired_at: Utc::now(),
            success: true,
            error: None,
        };

        if let Ok(mut history) = self.history.lock() {
            history.push(entry);
        } else {
            warn!("failed to lock watcher history for recording");
        }

        info!(
            task_id = %task.id,
            watcher = %watcher_name,
            "watcher agent executed"
        );

        Ok(polkagent_scheduler::TaskResult {
            success: true,
            message: Some(format!("watcher '{watcher_name}' dispatched")),
            completed_at: Utc::now(),
            duration_ms: 0,
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use polkagent_grant::policy::Effect;
    use polkagent_scheduler::TaskStore;

    fn make_watcher_config(name: &str) -> WatcherConfig {
        WatcherConfig {
            name: name.to_owned(),
            agent_id: "agent-test".to_owned(),
            schedule: WatcherSchedule::Interval { every_secs: 300 },
            cedar_policy: "test-read-only".to_owned(),
            enabled: true,
            read_only: true,
            timeout_secs: 120,
        }
    }

    // ----- Test 1: Manager creation -----

    #[test]
    fn manager_can_be_created_and_is_debuggable() {
        let manager = WatcherAgentManager::new(Duration::from_secs(10), true);
        let debug = format!("{manager:?}");
        assert!(debug.contains("WatcherAgentManager"));
        assert!(debug.contains("timeout_enforcer_active"));
    }

    // ----- Test 2: Registration requires TimeoutEnforcer -----

    #[tokio::test]
    async fn register_fails_without_timeout_enforcer() {
        let mut manager = WatcherAgentManager::new(Duration::from_secs(10), false);
        let config = make_watcher_config("test-watcher");
        let result = manager.register_watcher(&config).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("TimeoutEnforcer"));
    }

    // ----- Test 3: Successful registration -----

    #[tokio::test]
    async fn register_watcher_succeeds() {
        let mut manager = WatcherAgentManager::new(Duration::from_secs(1), true);
        let config = make_watcher_config("balance-monitor");
        let task_id = manager.register_watcher(&config).await.expect("register");

        assert!(manager
            .registered_watchers()
            .contains(&"balance-monitor".to_owned()));
        assert!(manager.watcher_policy("balance-monitor").is_some());
        assert!(manager.task_ids.contains_key("balance-monitor"));
        let _ = task_id; // just verify it's returned
    }

    // ----- Test 4: Read-only policy denies writes -----

    #[test]
    fn read_only_policy_has_deny_writes_rule() {
        let policy_set = build_watcher_policy("test-policy", true);
        let rules = policy_set.rules;
        assert!(
            rules.iter().any(|r| r.id.contains("deny-writes")),
            "expected deny-writes rule in read-only policy"
        );
        // Also has allow-read.
        assert!(
            rules.iter().any(|r| r.id.contains("allow-read")),
            "expected allow-read rule"
        );
    }

    // ----- Test 5: Non-read-only policy omits deny -----

    #[test]
    fn non_read_only_policy_omits_deny_writes() {
        let policy_set = build_watcher_policy("test-policy", false);
        let rules = policy_set.rules;
        assert!(
            !rules.iter().any(|r| r.id.contains("deny-writes")),
            "non-read-only policy should not have deny-writes rule"
        );
        assert!(
            rules.iter().any(|r| r.id.contains("allow-read")),
            "should still have allow-read rule"
        );
    }

    // ----- Test 6: Disabled watcher is skipped -----

    #[tokio::test]
    async fn disabled_watcher_rejected_on_register() {
        let mut manager = WatcherAgentManager::new(Duration::from_secs(1), true);
        let mut config = make_watcher_config("disabled-one");
        config.enabled = false;
        let result = manager.register_watcher(&config).await;
        assert!(result.is_err());
    }

    // ----- Test 7: register_all skips disabled watchers -----

    #[tokio::test]
    async fn register_all_skips_disabled() {
        let mut manager = WatcherAgentManager::new(Duration::from_secs(1), true);
        let mut enabled = make_watcher_config("enabled-1");
        enabled.enabled = true;
        let mut disabled = make_watcher_config("disabled-1");
        disabled.enabled = false;

        let count = manager
            .register_all(&[enabled, disabled])
            .await
            .expect("register_all");
        assert_eq!(count, 1);
        assert_eq!(manager.registered_watchers().len(), 1);
    }

    // ----- Test 8: Unregister watcher -----

    #[tokio::test]
    async fn unregister_watcher_removes_it() {
        let mut manager = WatcherAgentManager::new(Duration::from_secs(1), true);
        let config = make_watcher_config("remove-me");
        manager.register_watcher(&config).await.expect("register");
        assert_eq!(manager.registered_watchers().len(), 1);

        manager
            .unregister_watcher("remove-me")
            .await
            .expect("unregister");
        assert!(manager.registered_watchers().is_empty());
        assert!(manager.watcher_policy("remove-me").is_none());
    }

    // ----- Test 9: Unregister nonexistent fails -----

    #[tokio::test]
    async fn unregister_nonexistent_returns_error() {
        let mut manager = WatcherAgentManager::new(Duration::from_secs(1), true);
        let result = manager.unregister_watcher("ghost").await;
        assert!(result.is_err());
    }

    // ----- Test 10: Watcher fires and records history -----

    #[tokio::test]
    async fn watcher_fires_and_records_history() {
        let mut manager = WatcherAgentManager::new(Duration::from_secs(1), true);
        let config = make_watcher_config("fire-test");
        let task_id = manager.register_watcher(&config).await.expect("register");

        // Make the task due.
        let store = manager.store();
        let mut task = store.get_task(task_id).await.expect("get");
        task.next_run_at = Some(Utc::now() - chrono::Duration::seconds(1));
        task.status = polkagent_scheduler::task::TaskStatus::Active;
        store.update_task(task).await.expect("update");

        let executed = manager.poll_once().await.expect("poll");
        assert_eq!(executed, 1);

        let history = manager.history();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].watcher_name, "fire-test");
        assert!(history[0].read_only);
        assert!(history[0].success);
        assert_eq!(history[0].cedar_policy, "test-read-only");
    }

    // ----- Test 11: Schedule conversion -----

    #[test]
    fn interval_schedule_conversion() {
        let ws = WatcherSchedule::Interval { every_secs: 60 };
        let schedule = to_scheduler_schedule(&ws).expect("convert");
        match schedule {
            Schedule::Interval { every, .. } => {
                assert_eq!(every.num_seconds(), 60);
            }
            _ => panic!("expected Interval schedule"),
        }
    }

    #[test]
    fn cron_schedule_conversion() {
        let ws = WatcherSchedule::Cron {
            expr: "*/5 * * * *".to_owned(),
        };
        let schedule = to_scheduler_schedule(&ws).expect("convert");
        match schedule {
            Schedule::Cron(_) => {}
            _ => panic!("expected Cron schedule"),
        }
    }

    // ----- Test 12: Policy deny-overrides write attempt -----

    #[test]
    fn read_only_policy_deny_rules_are_deny_effect() {
        let policy_set = build_watcher_policy("secure-watcher", true);
        let deny_rules: Vec<_> = policy_set
            .rules
            .iter()
            .filter(|r| r.effect == Effect::Deny)
            .collect();
        assert_eq!(deny_rules.len(), 1);
        let deny = &deny_rules[0];
        assert!(deny.action_patterns.contains(&"chain.submit".to_owned()));
        assert!(deny.action_patterns.contains(&"chain.transfer".to_owned()));
        assert!(deny.action_patterns.contains(&"effect.*".to_owned()));
    }

    // ----- Test 13: Multiple watchers can coexist -----

    #[tokio::test]
    async fn multiple_watchers_can_be_registered() {
        let mut manager = WatcherAgentManager::new(Duration::from_secs(1), true);
        for i in 0..3 {
            let config = make_watcher_config(&format!("watcher-{i}"));
            manager.register_watcher(&config).await.expect("register");
        }
        assert_eq!(manager.registered_watchers().len(), 3);
    }

    // ----- Test 14: History empty initially -----

    #[test]
    fn history_is_empty_initially() {
        let manager = WatcherAgentManager::new(Duration::from_secs(1), true);
        assert!(manager.history().is_empty());
    }
}
