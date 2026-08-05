//! `polkagent-cloud-control` — Cloud control plane for the Polkagent platform.
//!
//! Provides the job queue, worker registration with heartbeat tracking, and
//! load-aware job assignment that together form the execution-plane control
//! surface described in PRD-11 §4.4.

#![forbid(unsafe_code)]
#![warn(
    missing_docs,
    clippy::pedantic,
    clippy::unwrap_used,
    clippy::expect_used
)]
#![allow(
    clippy::module_name_repetitions,
    clippy::must_use_candidate,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc
)]

use std::cmp::Reverse;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use polkagent_config::schema::DataRegion;
use polkagent_core::{RunId, WorkerId};
use polkagent_grant::policy::{
    evaluate, Condition, ContextAttribute, Effect, EvaluationContext, PolicyDecision, PolicyRule,
    PolicySet,
};

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors returned by the control plane.
#[derive(Debug, thiserror::Error)]
pub enum ControlError {
    /// The referenced worker was not found in the registry.
    #[error("worker not found: {0}")]
    WorkerNotFound(WorkerId),

    /// The referenced job was not found in the queue.
    #[error("job not found: {0}")]
    JobNotFound(String),

    /// No workers are available to accept jobs.
    #[error("no workers available")]
    NoWorkersAvailable,

    /// The worker does not have the required capability.
    #[error("worker {worker_id} lacks capability: {capability}")]
    MissingCapability {
        /// The worker that was checked.
        worker_id: WorkerId,
        /// The capability that was required.
        capability: String,
    },

    /// The job queue has reached its maximum capacity.
    #[error("job queue is full (capacity: {capacity})")]
    QueueFull {
        /// Maximum queue capacity.
        capacity: usize,
    },

    /// A job cannot be routed because no workers exist in the required region.
    #[error("data residency violation: job region {job_region} has no eligible workers (cross-region routing disabled)")]
    RegionViolation {
        /// The region the job requires.
        job_region: DataRegion,
    },
}

/// Convenience alias for control-plane results.
pub type Result<T> = std::result::Result<T, ControlError>;

// ---------------------------------------------------------------------------
// Job
// ---------------------------------------------------------------------------

/// Priority level for a queued job.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub enum JobPriority {
    /// Lowest priority — processed after all higher-priority jobs.
    Low = 0,
    /// Default priority.
    #[default]
    Normal = 1,
    /// Elevated priority — scheduled before Normal/Low.
    High = 2,
    /// Highest priority — processed as soon as capacity is available.
    Critical = 3,
}

/// Current status of a job in the queue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobStatus {
    /// Waiting in the queue for a worker.
    Pending,
    /// Assigned to a worker and currently running.
    Assigned {
        /// The worker executing this job.
        worker_id: WorkerId,
    },
    /// Completed successfully.
    Completed,
    /// Failed with an error message.
    Failed {
        /// Human-readable failure reason.
        reason: String,
    },
}

/// A job submitted to the control plane for execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    /// Unique job identifier.
    pub id: String,
    /// The run this job is associated with.
    pub run_id: RunId,
    /// Opaque payload describing the work to be done.
    pub payload: String,
    /// Required worker capability (if any).
    pub required_capability: Option<String>,
    /// Data-residency region of the submitting tenant. When set, the job may
    /// only be assigned to a worker in the same region unless cross-region
    /// routing is explicitly allowed.
    pub region: Option<DataRegion>,
    /// Job priority.
    pub priority: JobPriority,
    /// Current status.
    pub status: JobStatus,
    /// When the job was enqueued.
    pub created_at: DateTime<Utc>,
    /// When the job was last updated.
    pub updated_at: DateTime<Utc>,
}

impl Job {
    /// Create a new pending job.
    pub fn new(id: impl Into<String>, run_id: RunId, payload: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            id: id.into(),
            run_id,
            payload: payload.into(),
            required_capability: None,
            region: None,
            priority: JobPriority::default(),
            status: JobStatus::Pending,
            created_at: now,
            updated_at: now,
        }
    }
}

// ---------------------------------------------------------------------------
// Worker registration
// ---------------------------------------------------------------------------

/// Health status of a registered worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkerStatus {
    /// The worker is online and accepting jobs.
    Active,
    /// The worker is draining — it will finish current jobs but not accept new
    /// ones.
    Draining,
    /// The worker has not sent a heartbeat within the expected interval.
    Stale,
    /// The worker has been deregistered.
    Offline,
}

/// A registered worker in the control plane.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerRecord {
    /// Unique worker identifier.
    pub id: WorkerId,
    /// Human-readable name or hostname.
    pub name: String,
    /// Set of capabilities this worker advertises.
    pub capabilities: Vec<String>,
    /// Data-residency region where this worker is deployed.
    pub region: Option<DataRegion>,
    /// Maximum number of concurrent jobs.
    pub max_concurrent_jobs: u32,
    /// Number of currently assigned jobs.
    pub active_jobs: u32,
    /// Current health status.
    pub status: WorkerStatus,
    /// When this worker first registered.
    pub registered_at: DateTime<Utc>,
    /// Timestamp of the last heartbeat received.
    pub last_heartbeat: DateTime<Utc>,
}

impl WorkerRecord {
    /// Create a new worker record.
    pub fn new(
        id: WorkerId,
        name: impl Into<String>,
        capabilities: Vec<String>,
        max_concurrent_jobs: u32,
    ) -> Self {
        let now = Utc::now();
        Self {
            id,
            name: name.into(),
            capabilities,
            region: None,
            max_concurrent_jobs,
            active_jobs: 0,
            status: WorkerStatus::Active,
            registered_at: now,
            last_heartbeat: now,
        }
    }

    /// Returns `true` if the worker can accept a new job.
    pub fn has_capacity(&self) -> bool {
        self.status == WorkerStatus::Active && self.active_jobs < self.max_concurrent_jobs
    }

    /// Returns the number of free slots.
    pub fn free_slots(&self) -> u32 {
        if self.status != WorkerStatus::Active {
            return 0;
        }
        self.max_concurrent_jobs.saturating_sub(self.active_jobs)
    }

    /// Returns `true` if the worker has a given capability.
    pub fn has_capability(&self, cap: &str) -> bool {
        self.capabilities.iter().any(|c| c == cap)
    }
}

// ---------------------------------------------------------------------------
// Control plane
// ---------------------------------------------------------------------------

/// Configuration for the control plane.
#[derive(Debug, Clone)]
pub struct ControlPlaneConfig {
    /// Maximum number of pending jobs allowed in the queue.
    pub max_queue_size: usize,
    /// Duration after which a worker without a heartbeat is marked stale.
    pub heartbeat_timeout: Duration,
    /// When `true`, jobs may be assigned to workers in a different region than
    /// the job's declared region. Default: `false`.
    pub allow_cross_region: bool,
}

impl Default for ControlPlaneConfig {
    fn default() -> Self {
        Self {
            max_queue_size: 10_000,
            heartbeat_timeout: Duration::from_secs(30),
            allow_cross_region: false,
        }
    }
}

/// Build the Cedar-style policy set that enforces data-residency constraints.
///
/// The returned [`PolicySet`] contains two rules:
/// 1. A deny rule that fires when the job region does not match the worker
///    region (cross-region routing blocked).
/// 2. An allow rule that permits all `cloud.assign_job` actions (baseline
///    permission so that same-region assignments succeed).
///
/// Deny-overrides semantics ensure the deny rule wins when regions differ.
#[must_use]
pub fn data_residency_policy(worker_region: DataRegion) -> PolicySet {
    PolicySet::new(vec![
        // Deny when the job's region differs from the worker's region.
        PolicyRule {
            id: "deny-cross-region-routing".to_owned(),
            effect: Effect::Deny,
            action_patterns: vec!["cloud.assign_job".to_owned()],
            resource_patterns: vec!["**".to_owned()],
            conditions: HashMap::default(),
            abac_condition: Some(Condition::Not {
                inner: Box::new(Condition::Equals {
                    attr: "job.region".to_owned(),
                    value: worker_region.to_string(),
                }),
            }),
        },
        // Allow baseline — only reached when the deny rule above did not fire.
        PolicyRule {
            id: "allow-same-region-routing".to_owned(),
            effect: Effect::Allow,
            action_patterns: vec!["cloud.assign_job".to_owned()],
            resource_patterns: vec!["**".to_owned()],
            conditions: HashMap::default(),
            abac_condition: None,
        },
    ])
}

/// Evaluate the data-residency policy for a specific job→worker assignment.
///
/// Returns `true` when the assignment is permitted, `false` when the policy
/// denies it.
fn check_residency_policy(job_region: DataRegion, worker_region: DataRegion) -> bool {
    let policy = data_residency_policy(worker_region);
    let ctx = EvaluationContext::default().with_attribute(
        "job.region",
        ContextAttribute::String(job_region.to_string()),
    );

    let decision = evaluate(&policy, "cloud.assign_job", "job", &ctx);
    matches!(decision, PolicyDecision::Allow)
}

/// The cloud control plane.
///
/// Manages a job queue backed by an in-memory store (SQLite-compatible
/// schema planned for a future iteration), worker registration with heartbeat
/// tracking, and load-aware job assignment.
#[derive(Debug, Clone)]
pub struct ControlPlane {
    inner: Arc<Mutex<ControlPlaneInner>>,
}

#[derive(Debug)]
struct ControlPlaneInner {
    config: ControlPlaneConfig,
    workers: HashMap<WorkerId, WorkerRecord>,
    pending_jobs: VecDeque<Job>,
    all_jobs: HashMap<String, Job>,
}

impl ControlPlane {
    /// Create a new control plane with the given configuration.
    pub fn new(config: ControlPlaneConfig) -> Self {
        Self {
            inner: Arc::new(Mutex::new(ControlPlaneInner {
                config,
                workers: HashMap::new(),
                pending_jobs: VecDeque::new(),
                all_jobs: HashMap::new(),
            })),
        }
    }

    /// Create a control plane with default configuration.
    pub fn with_defaults() -> Self {
        Self::new(ControlPlaneConfig::default())
    }

    // -- Worker management ---------------------------------------------------

    /// Register a new worker.
    pub async fn register_worker(&self, record: WorkerRecord) -> Result<()> {
        let mut inner = self.inner.lock().await;
        tracing::info!(worker_id = %record.id, name = %record.name, "worker registered");
        inner.workers.insert(record.id, record);
        Ok(())
    }

    /// Remove a worker from the registry.
    pub async fn deregister_worker(&self, worker_id: WorkerId) -> Result<()> {
        let mut inner = self.inner.lock().await;
        if inner.workers.remove(&worker_id).is_none() {
            return Err(ControlError::WorkerNotFound(worker_id));
        }
        tracing::info!(worker_id = %worker_id, "worker deregistered");
        Ok(())
    }

    /// Record a heartbeat from a worker.
    pub async fn heartbeat(&self, worker_id: WorkerId) -> Result<()> {
        let mut inner = self.inner.lock().await;
        let record = inner
            .workers
            .get_mut(&worker_id)
            .ok_or(ControlError::WorkerNotFound(worker_id))?;
        record.last_heartbeat = Utc::now();
        if record.status == WorkerStatus::Stale {
            record.status = WorkerStatus::Active;
        }
        Ok(())
    }

    /// Mark workers that have not sent a heartbeat recently as stale.
    pub async fn sweep_stale_workers(&self) -> Vec<WorkerId> {
        let mut inner = self.inner.lock().await;
        let cutoff = Utc::now() - inner.config.heartbeat_timeout;
        let mut stale = Vec::new();
        for record in inner.workers.values_mut() {
            if record.status == WorkerStatus::Active && record.last_heartbeat < cutoff {
                record.status = WorkerStatus::Stale;
                stale.push(record.id);
            }
        }
        stale
    }

    /// Return a snapshot of all registered workers.
    pub async fn list_workers(&self) -> Vec<WorkerRecord> {
        let inner = self.inner.lock().await;
        inner.workers.values().cloned().collect()
    }

    /// Get a single worker by ID.
    pub async fn get_worker(&self, worker_id: WorkerId) -> Result<WorkerRecord> {
        let inner = self.inner.lock().await;
        inner
            .workers
            .get(&worker_id)
            .cloned()
            .ok_or(ControlError::WorkerNotFound(worker_id))
    }

    // -- Job queue -----------------------------------------------------------

    /// Enqueue a new job.
    pub async fn enqueue(&self, job: Job) -> Result<()> {
        let mut inner = self.inner.lock().await;
        if inner.pending_jobs.len() >= inner.config.max_queue_size {
            return Err(ControlError::QueueFull {
                capacity: inner.config.max_queue_size,
            });
        }
        tracing::debug!(job_id = %job.id, run_id = %job.run_id, "job enqueued");
        inner.all_jobs.insert(job.id.clone(), job.clone());
        inner.pending_jobs.push_back(job);
        Ok(())
    }

    /// Return the current number of pending jobs.
    pub async fn pending_count(&self) -> usize {
        let inner = self.inner.lock().await;
        inner.pending_jobs.len()
    }

    /// Look up a job by its ID.
    pub async fn get_job(&self, job_id: &str) -> Result<Job> {
        let inner = self.inner.lock().await;
        inner
            .all_jobs
            .get(job_id)
            .cloned()
            .ok_or_else(|| ControlError::JobNotFound(job_id.to_owned()))
    }

    /// Mark a job as completed.
    pub async fn complete_job(&self, job_id: &str) -> Result<()> {
        let mut inner = self.inner.lock().await;

        let assigned_worker = inner
            .all_jobs
            .get(job_id)
            .ok_or_else(|| ControlError::JobNotFound(job_id.to_owned()))
            .map(|job| match &job.status {
                JobStatus::Assigned { worker_id } => Some(*worker_id),
                _ => None,
            })?;

        if let Some(wid) = assigned_worker {
            if let Some(w) = inner.workers.get_mut(&wid) {
                w.active_jobs = w.active_jobs.saturating_sub(1);
            }
        }

        let job = inner
            .all_jobs
            .get_mut(job_id)
            .ok_or_else(|| ControlError::JobNotFound(job_id.to_owned()))?;
        job.status = JobStatus::Completed;
        job.updated_at = Utc::now();
        Ok(())
    }

    /// Mark a job as failed.
    pub async fn fail_job(&self, job_id: &str, reason: impl Into<String>) -> Result<()> {
        let mut inner = self.inner.lock().await;

        let assigned_worker = inner
            .all_jobs
            .get(job_id)
            .ok_or_else(|| ControlError::JobNotFound(job_id.to_owned()))
            .map(|job| match &job.status {
                JobStatus::Assigned { worker_id } => Some(*worker_id),
                _ => None,
            })?;

        if let Some(wid) = assigned_worker {
            if let Some(w) = inner.workers.get_mut(&wid) {
                w.active_jobs = w.active_jobs.saturating_sub(1);
            }
        }

        let job = inner
            .all_jobs
            .get_mut(job_id)
            .ok_or_else(|| ControlError::JobNotFound(job_id.to_owned()))?;
        job.status = JobStatus::Failed {
            reason: reason.into(),
        };
        job.updated_at = Utc::now();
        Ok(())
    }

    // -- Job assignment ------------------------------------------------------

    /// Assign the next eligible pending job to the best available worker.
    ///
    /// Assignment strategy:
    /// 1. Pick the highest-priority pending job.
    /// 2. Find active workers with capacity.
    /// 3. If the job requires a capability, filter to capable workers.
    /// 4. If the job has a region and cross-region routing is disabled, filter
    ///    to workers in the same region (enforced via Cedar policy).
    /// 5. Among remaining candidates, pick the one with the most free slots
    ///    (least loaded).
    pub async fn assign_next(&self) -> Result<(Job, WorkerId)> {
        let mut inner = self.inner.lock().await;

        // Sort pending jobs by priority (highest first).
        let pending: Vec<_> = inner.pending_jobs.drain(..).collect();
        let mut sorted = pending;
        sorted.sort_by_key(|job| Reverse(job.priority));

        let allow_cross = inner.config.allow_cross_region;

        for job in &sorted {
            // Find eligible workers.
            let mut candidates: Vec<&WorkerRecord> = inner
                .workers
                .values()
                .filter(|w| w.has_capacity())
                .collect();

            if let Some(ref cap) = job.required_capability {
                candidates.retain(|w| w.has_capability(cap));
            }

            // Region-aware filtering: when cross-region routing is disabled
            // and the job declares a region, only workers in the same region
            // are eligible.  The check is delegated to the Cedar-style policy
            // evaluator so that the constraint is expressed as a formal policy
            // rule rather than ad-hoc code.
            if let Some(job_region) = job.region {
                if !allow_cross {
                    candidates.retain(|w| {
                        w.region.is_some_and(|worker_region| {
                            check_residency_policy(job_region, worker_region)
                        })
                    });
                }
            }

            // Pick least-loaded (most free slots).
            candidates.sort_by_key(|worker| Reverse(worker.free_slots()));

            if let Some(best) = candidates.first() {
                let worker_id = best.id;
                let job_id = job.id.clone();

                // Update worker load.
                if let Some(w) = inner.workers.get_mut(&worker_id) {
                    w.active_jobs += 1;
                }

                // Update job status.
                if let Some(j) = inner.all_jobs.get_mut(&job_id) {
                    j.status = JobStatus::Assigned { worker_id };
                    j.updated_at = Utc::now();
                }

                // Put remaining jobs back.
                for remaining in sorted.into_iter().filter(|j| j.id != job_id) {
                    inner.pending_jobs.push_back(remaining);
                }

                let assigned = inner
                    .all_jobs
                    .get(&job_id)
                    .cloned()
                    .ok_or(ControlError::JobNotFound(job_id))?;

                return Ok((assigned, worker_id));
            }
        }

        // No assignment was possible — put all jobs back.
        for job in sorted {
            inner.pending_jobs.push_back(job);
        }

        // Distinguish "no workers at all" from "region violation".
        if let Some(first_job) = inner.pending_jobs.front() {
            if let Some(job_region) = first_job.region {
                if !allow_cross {
                    let any_in_region =
                        inner.workers.values().any(|w| w.region == Some(job_region));
                    if !any_in_region {
                        return Err(ControlError::RegionViolation { job_region });
                    }
                }
            }
        }

        Err(ControlError::NoWorkersAvailable)
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
// Assertion-oriented unit tests intentionally unwrap fixture lookups and assignments.
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use polkagent_core::RunId;

    fn make_worker(name: &str, max_jobs: u32, caps: Vec<&str>) -> WorkerRecord {
        WorkerRecord::new(
            WorkerId::new(),
            name,
            caps.into_iter().map(String::from).collect(),
            max_jobs,
        )
    }

    fn make_job(id: &str) -> Job {
        Job::new(id, RunId::new(), format!("payload-{id}"))
    }

    // --- Job / Worker basics -----------------------------------------------

    #[test]
    fn job_new_is_pending() {
        let job = make_job("j1");
        assert_eq!(job.status, JobStatus::Pending);
        assert_eq!(job.priority, JobPriority::Normal);
    }

    #[test]
    fn worker_has_capacity_when_active_and_below_max() {
        let mut w = make_worker("w1", 2, vec![]);
        assert!(w.has_capacity());
        w.active_jobs = 2;
        assert!(!w.has_capacity());
    }

    #[test]
    fn worker_free_slots_zero_when_draining() {
        let mut w = make_worker("w1", 4, vec![]);
        w.status = WorkerStatus::Draining;
        assert_eq!(w.free_slots(), 0);
    }

    #[test]
    fn worker_has_capability_check() {
        let w = make_worker("w1", 1, vec!["gpu", "arm64"]);
        assert!(w.has_capability("gpu"));
        assert!(!w.has_capability("fpga"));
    }

    #[test]
    fn job_priority_ordering() {
        assert!(JobPriority::Critical > JobPriority::High);
        assert!(JobPriority::High > JobPriority::Normal);
        assert!(JobPriority::Normal > JobPriority::Low);
    }

    // --- Control plane operations ------------------------------------------

    #[tokio::test]
    async fn register_and_list_workers() {
        let cp = ControlPlane::with_defaults();
        let w = make_worker("node-1", 4, vec![]);
        cp.register_worker(w.clone()).await.ok();
        let workers = cp.list_workers().await;
        assert_eq!(workers.len(), 1);
        assert_eq!(workers[0].name, "node-1");
    }

    #[tokio::test]
    async fn deregister_unknown_worker_fails() {
        let cp = ControlPlane::with_defaults();
        let result = cp.deregister_worker(WorkerId::new()).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn enqueue_and_pending_count() {
        let cp = ControlPlane::with_defaults();
        assert_eq!(cp.pending_count().await, 0);
        cp.enqueue(make_job("j1")).await.ok();
        cp.enqueue(make_job("j2")).await.ok();
        assert_eq!(cp.pending_count().await, 2);
    }

    #[tokio::test]
    async fn enqueue_respects_max_queue_size() {
        let cp = ControlPlane::new(ControlPlaneConfig {
            max_queue_size: 1,
            ..Default::default()
        });
        cp.enqueue(make_job("j1")).await.ok();
        let result = cp.enqueue(make_job("j2")).await;
        assert!(matches!(result, Err(ControlError::QueueFull { .. })));
    }

    #[tokio::test]
    async fn assign_next_picks_least_loaded_worker() {
        let cp = ControlPlane::with_defaults();

        let mut busy = make_worker("busy", 4, vec![]);
        busy.active_jobs = 3;
        let idle = make_worker("idle", 4, vec![]);

        cp.register_worker(busy).await.ok();
        cp.register_worker(idle.clone()).await.ok();
        cp.enqueue(make_job("j1")).await.ok();

        let (assigned, worker_id) = cp.assign_next().await.ok().unwrap();
        assert_eq!(assigned.id, "j1");
        assert_eq!(worker_id, idle.id);
    }

    #[tokio::test]
    async fn assign_next_filters_by_capability() {
        let cp = ControlPlane::with_defaults();
        let gpu_worker = make_worker("gpu-node", 4, vec!["gpu"]);
        let cpu_worker = make_worker("cpu-node", 4, vec!["cpu"]);

        cp.register_worker(gpu_worker.clone()).await.ok();
        cp.register_worker(cpu_worker).await.ok();

        let mut job = make_job("gpu-job");
        job.required_capability = Some("gpu".into());
        cp.enqueue(job).await.ok();

        let (_, worker_id) = cp.assign_next().await.ok().unwrap();
        assert_eq!(worker_id, gpu_worker.id);
    }

    #[tokio::test]
    async fn assign_next_fails_when_no_workers() {
        let cp = ControlPlane::with_defaults();
        cp.enqueue(make_job("j1")).await.ok();
        let result = cp.assign_next().await;
        assert!(matches!(result, Err(ControlError::NoWorkersAvailable)));
    }

    #[tokio::test]
    async fn complete_job_decrements_worker_load() {
        let cp = ControlPlane::with_defaults();
        let w = make_worker("w1", 4, vec![]);
        let wid = w.id;
        cp.register_worker(w).await.ok();
        cp.enqueue(make_job("j1")).await.ok();
        cp.assign_next().await.ok();

        let before = cp.get_worker(wid).await.ok().unwrap();
        assert_eq!(before.active_jobs, 1);

        cp.complete_job("j1").await.ok();

        let after = cp.get_worker(wid).await.ok().unwrap();
        assert_eq!(after.active_jobs, 0);

        let job = cp.get_job("j1").await.ok().unwrap();
        assert_eq!(job.status, JobStatus::Completed);
    }

    #[tokio::test]
    async fn fail_job_updates_status() {
        let cp = ControlPlane::with_defaults();
        let w = make_worker("w1", 4, vec![]);
        cp.register_worker(w).await.ok();
        cp.enqueue(make_job("j1")).await.ok();
        cp.assign_next().await.ok();

        cp.fail_job("j1", "timeout").await.ok();
        let job = cp.get_job("j1").await.ok().unwrap();
        assert!(matches!(job.status, JobStatus::Failed { reason } if reason == "timeout"));
    }

    #[tokio::test]
    async fn heartbeat_updates_timestamp() {
        let cp = ControlPlane::with_defaults();
        let w = make_worker("w1", 4, vec![]);
        let wid = w.id;
        let original_hb = w.last_heartbeat;
        cp.register_worker(w).await.ok();

        // Small sleep to ensure timestamp changes
        tokio::time::sleep(Duration::from_millis(10)).await;
        cp.heartbeat(wid).await.ok();

        let updated = cp.get_worker(wid).await.ok().unwrap();
        assert!(updated.last_heartbeat >= original_hb);
    }

    #[tokio::test]
    async fn sweep_stale_marks_workers_without_recent_heartbeat() {
        let cp = ControlPlane::new(ControlPlaneConfig {
            heartbeat_timeout: Duration::from_millis(1),
            ..Default::default()
        });
        let w = make_worker("w1", 4, vec![]);
        let wid = w.id;
        cp.register_worker(w).await.ok();

        tokio::time::sleep(Duration::from_millis(10)).await;

        let stale = cp.sweep_stale_workers().await;
        assert_eq!(stale, vec![wid]);
        let record = cp.get_worker(wid).await.ok().unwrap();
        assert_eq!(record.status, WorkerStatus::Stale);
    }

    #[tokio::test]
    async fn heartbeat_recovers_stale_worker() {
        let cp = ControlPlane::new(ControlPlaneConfig {
            heartbeat_timeout: Duration::from_millis(1),
            ..Default::default()
        });
        let w = make_worker("w1", 4, vec![]);
        let wid = w.id;
        cp.register_worker(w).await.ok();

        tokio::time::sleep(Duration::from_millis(10)).await;
        cp.sweep_stale_workers().await;

        let stale = cp.get_worker(wid).await.ok().unwrap();
        assert_eq!(stale.status, WorkerStatus::Stale);

        cp.heartbeat(wid).await.ok();
        let recovered = cp.get_worker(wid).await.ok().unwrap();
        assert_eq!(recovered.status, WorkerStatus::Active);
    }

    #[tokio::test]
    async fn assign_respects_priority_order() {
        let cp = ControlPlane::with_defaults();
        let w = make_worker("w1", 1, vec![]);
        cp.register_worker(w).await.ok();

        let mut low = make_job("low");
        low.priority = JobPriority::Low;
        let mut high = make_job("high");
        high.priority = JobPriority::High;

        // Enqueue low first, then high.
        cp.enqueue(low).await.ok();
        cp.enqueue(high).await.ok();

        // High-priority job should be assigned first.
        let (assigned, _) = cp.assign_next().await.ok().unwrap();
        assert_eq!(assigned.id, "high");
    }

    // --- Data residency tests ------------------------------------------------

    fn make_regional_worker(name: &str, max_jobs: u32, region: DataRegion) -> WorkerRecord {
        let mut w = make_worker(name, max_jobs, vec![]);
        w.region = Some(region);
        w
    }

    fn make_regional_job(id: &str, region: DataRegion) -> Job {
        let mut j = make_job(id);
        j.region = Some(region);
        j
    }

    #[tokio::test]
    async fn eu_job_cannot_route_to_us_worker() {
        let cp = ControlPlane::with_defaults(); // cross-region disabled by default
        let us_worker = make_regional_worker("us-worker", 4, DataRegion::Us);
        cp.register_worker(us_worker).await.ok();

        let eu_job = make_regional_job("eu-job-1", DataRegion::Eu);
        cp.enqueue(eu_job).await.ok();

        let result = cp.assign_next().await;
        assert!(
            matches!(result, Err(ControlError::RegionViolation { job_region }) if job_region == DataRegion::Eu),
            "EU job must not be assigned to a US worker"
        );
    }

    #[tokio::test]
    async fn eu_job_routes_to_eu_worker() {
        let cp = ControlPlane::with_defaults();
        let eu_worker = make_regional_worker("eu-worker", 4, DataRegion::Eu);
        let eu_wid = eu_worker.id;
        cp.register_worker(eu_worker).await.ok();

        let eu_job = make_regional_job("eu-job-2", DataRegion::Eu);
        cp.enqueue(eu_job).await.ok();

        let (assigned, worker_id) = cp.assign_next().await.ok().unwrap();
        assert_eq!(assigned.id, "eu-job-2");
        assert_eq!(worker_id, eu_wid);
    }

    #[tokio::test]
    async fn cross_region_override_allows_routing() {
        let cp = ControlPlane::new(ControlPlaneConfig {
            allow_cross_region: true,
            ..Default::default()
        });
        let us_worker = make_regional_worker("us-worker", 4, DataRegion::Us);
        let us_wid = us_worker.id;
        cp.register_worker(us_worker).await.ok();

        let eu_job = make_regional_job("eu-job-3", DataRegion::Eu);
        cp.enqueue(eu_job).await.ok();

        let (assigned, worker_id) = cp.assign_next().await.ok().unwrap();
        assert_eq!(assigned.id, "eu-job-3");
        assert_eq!(
            worker_id, us_wid,
            "cross-region override should allow assignment"
        );
    }

    #[tokio::test]
    async fn regionless_job_can_route_to_any_worker() {
        let cp = ControlPlane::with_defaults();
        let eu_worker = make_regional_worker("eu-worker", 4, DataRegion::Eu);
        let eu_wid = eu_worker.id;
        cp.register_worker(eu_worker).await.ok();

        // Job without a region should still be assignable.
        let job = make_job("no-region-job");
        cp.enqueue(job).await.ok();

        let (assigned, worker_id) = cp.assign_next().await.ok().unwrap();
        assert_eq!(assigned.id, "no-region-job");
        assert_eq!(worker_id, eu_wid);
    }

    #[tokio::test]
    async fn residency_policy_denies_mismatched_regions() {
        assert!(
            !check_residency_policy(DataRegion::Eu, DataRegion::Us),
            "EU→US must be denied"
        );
        assert!(
            check_residency_policy(DataRegion::Eu, DataRegion::Eu),
            "EU→EU must be allowed"
        );
        assert!(
            !check_residency_policy(DataRegion::Ap, DataRegion::Ca),
            "AP→CA must be denied"
        );
    }

    #[tokio::test]
    async fn multiple_regions_picks_correct_worker() {
        let cp = ControlPlane::with_defaults();
        let eu_worker = make_regional_worker("eu-node", 4, DataRegion::Eu);
        let us_worker = make_regional_worker("us-node", 4, DataRegion::Us);
        let eu_wid = eu_worker.id;
        let us_wid = us_worker.id;

        cp.register_worker(eu_worker).await.ok();
        cp.register_worker(us_worker).await.ok();

        let eu_job = make_regional_job("j-eu", DataRegion::Eu);
        cp.enqueue(eu_job).await.ok();
        let (_, wid) = cp.assign_next().await.ok().unwrap();
        assert_eq!(wid, eu_wid, "EU job must go to EU worker");

        let us_job = make_regional_job("j-us", DataRegion::Us);
        cp.enqueue(us_job).await.ok();
        let (_, wid) = cp.assign_next().await.ok().unwrap();
        assert_eq!(wid, us_wid, "US job must go to US worker");
    }
}
