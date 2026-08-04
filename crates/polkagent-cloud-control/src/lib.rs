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

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use polkagent_core::{RunId, WorkerId};

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
}

/// Convenience alias for control-plane results.
pub type Result<T> = std::result::Result<T, ControlError>;

// ---------------------------------------------------------------------------
// Job
// ---------------------------------------------------------------------------

/// Priority level for a queued job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum JobPriority {
    /// Lowest priority — processed after all higher-priority jobs.
    Low = 0,
    /// Default priority.
    Normal = 1,
    /// Elevated priority — scheduled before Normal/Low.
    High = 2,
    /// Highest priority — processed as soon as capacity is available.
    Critical = 3,
}

impl Default for JobPriority {
    fn default() -> Self {
        Self::Normal
    }
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
}

impl Default for ControlPlaneConfig {
    fn default() -> Self {
        Self {
            max_queue_size: 10_000,
            heartbeat_timeout: Duration::from_secs(30),
        }
    }
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
            .and_then(|j| match &j.status {
                JobStatus::Assigned { worker_id } => Ok(Some(*worker_id)),
                _ => Ok(None),
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
            .and_then(|j| match &j.status {
                JobStatus::Assigned { worker_id } => Ok(Some(*worker_id)),
                _ => Ok(None),
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
    /// 4. Among remaining candidates, pick the one with the most free slots
    ///    (least loaded).
    pub async fn assign_next(&self) -> Result<(Job, WorkerId)> {
        let mut inner = self.inner.lock().await;

        // Sort pending jobs by priority (highest first).
        let pending: Vec<_> = inner.pending_jobs.drain(..).collect();
        let mut sorted = pending;
        sorted.sort_by(|a, b| b.priority.cmp(&a.priority));

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

            // Pick least-loaded (most free slots).
            candidates.sort_by(|a, b| b.free_slots().cmp(&a.free_slots()));

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

                let assigned = inner.all_jobs.get(&job_id).cloned().ok_or_else(|| {
                    ControlError::JobNotFound(job_id)
                })?;

                return Ok((assigned, worker_id));
            }
        }

        // No assignment was possible — put all jobs back.
        for job in sorted {
            inner.pending_jobs.push_back(job);
        }

        Err(ControlError::NoWorkersAvailable)
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
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
        let w_gpu = make_worker("gpu-node", 4, vec!["gpu"]);
        let w_cpu = make_worker("cpu-node", 4, vec!["cpu"]);

        cp.register_worker(w_gpu.clone()).await.ok();
        cp.register_worker(w_cpu).await.ok();

        let mut job = make_job("gpu-job");
        job.required_capability = Some("gpu".into());
        cp.enqueue(job).await.ok();

        let (_, worker_id) = cp.assign_next().await.ok().unwrap();
        assert_eq!(worker_id, w_gpu.id);
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
}
