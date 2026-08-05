//! `polkagent-cloud-worker` — Cloud worker plane for the Polkagent platform.
//!
//! Registers with the control plane on startup, receives job assignments,
//! executes runs, sends periodic heartbeats, and supports graceful drain
//! as described in PRD-11 §4.3.

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

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, Notify};

use polkagent_cloud_control::{ControlPlane, Job, WorkerRecord, WorkerStatus};
use polkagent_core::WorkerId;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors returned by the worker plane.
#[derive(Debug, thiserror::Error)]
pub enum WorkerError {
    /// The worker has not been registered with the control plane yet.
    #[error("worker not registered")]
    NotRegistered,

    /// The worker is in drain mode and cannot accept new jobs.
    #[error("worker is draining")]
    Draining,

    /// The control plane returned an error.
    #[error("control plane error: {0}")]
    Control(#[from] polkagent_cloud_control::ControlError),

    /// The job executor returned an error.
    #[error("job execution failed: {0}")]
    Execution(String),
}

/// Convenience alias for worker-plane results.
pub type Result<T> = std::result::Result<T, WorkerError>;

// ---------------------------------------------------------------------------
// Job executor trait
// ---------------------------------------------------------------------------

/// Trait for executing jobs received from the control plane.
///
/// Implementors perform the actual run execution — invoking harness calls,
/// managing turns, etc. The worker plane itself is agnostic to the execution
/// strategy.
#[async_trait]
pub trait JobExecutor: Send + Sync {
    /// Execute a job and return a result description.
    async fn execute(&self, job: &Job) -> std::result::Result<String, String>;
}

// ---------------------------------------------------------------------------
// Worker state
// ---------------------------------------------------------------------------

/// Snapshot of the worker's current operational state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerSnapshot {
    /// Worker identity.
    pub worker_id: WorkerId,
    /// Worker name.
    pub name: String,
    /// Whether the worker is draining.
    pub draining: bool,
    /// Number of jobs currently being executed.
    pub active_jobs: u32,
    /// Total jobs completed since startup.
    pub completed_jobs: u64,
    /// Total jobs failed since startup.
    pub failed_jobs: u64,
    /// When the worker started.
    pub started_at: DateTime<Utc>,
    /// Last heartbeat sent.
    pub last_heartbeat: Option<DateTime<Utc>>,
}

// ---------------------------------------------------------------------------
// Worker configuration
// ---------------------------------------------------------------------------

/// Configuration for the cloud worker.
#[derive(Debug, Clone)]
pub struct WorkerConfig {
    /// Human-readable worker name.
    pub name: String,
    /// Capabilities advertised to the control plane.
    pub capabilities: Vec<String>,
    /// Maximum concurrent jobs this worker will accept.
    pub max_concurrent_jobs: u32,
    /// Interval between heartbeat pings.
    pub heartbeat_interval: Duration,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            name: "worker".into(),
            capabilities: Vec::new(),
            max_concurrent_jobs: 4,
            heartbeat_interval: Duration::from_secs(10),
        }
    }
}

// ---------------------------------------------------------------------------
// Cloud worker
// ---------------------------------------------------------------------------

/// The cloud worker agent.
///
/// On startup it registers with the control plane, then enters a loop that:
/// 1. Sends periodic heartbeats.
/// 2. Polls for job assignments.
/// 3. Executes assigned jobs via a [`JobExecutor`].
/// 4. Supports graceful drain — stops accepting new work but finishes
///    in-flight jobs before shutting down.
#[derive(Clone)]
pub struct CloudWorker {
    inner: Arc<Mutex<WorkerInner>>,
    drain_notify: Arc<Notify>,
}

struct WorkerInner {
    config: WorkerConfig,
    worker_id: Option<WorkerId>,
    control: ControlPlane,
    draining: bool,
    active_jobs: u32,
    completed_jobs: u64,
    failed_jobs: u64,
    started_at: DateTime<Utc>,
    last_heartbeat: Option<DateTime<Utc>>,
}

impl CloudWorker {
    /// Create a new cloud worker connected to the given control plane.
    pub fn new(control: ControlPlane, config: WorkerConfig) -> Self {
        Self {
            inner: Arc::new(Mutex::new(WorkerInner {
                config,
                worker_id: None,
                control,
                draining: false,
                active_jobs: 0,
                completed_jobs: 0,
                failed_jobs: 0,
                started_at: Utc::now(),
                last_heartbeat: None,
            })),
            drain_notify: Arc::new(Notify::new()),
        }
    }

    /// Register this worker with the control plane.
    pub async fn register(&self) -> Result<WorkerId> {
        let mut inner = self.inner.lock().await;
        let id = WorkerId::new();
        let record = WorkerRecord::new(
            id,
            &inner.config.name,
            inner.config.capabilities.clone(),
            inner.config.max_concurrent_jobs,
        );
        inner.control.register_worker(record).await?;
        inner.worker_id = Some(id);
        tracing::info!(worker_id = %id, "cloud worker registered");
        Ok(id)
    }

    /// Send a heartbeat to the control plane.
    pub async fn send_heartbeat(&self) -> Result<()> {
        let mut inner = self.inner.lock().await;
        let wid = inner.worker_id.ok_or(WorkerError::NotRegistered)?;
        inner.control.heartbeat(wid).await?;
        inner.last_heartbeat = Some(Utc::now());
        Ok(())
    }

    /// Poll the control plane for a job, execute it, and report the result.
    pub async fn poll_and_execute(&self, executor: &dyn JobExecutor) -> Result<Option<String>> {
        // Check state without holding lock during execution.
        let (control, draining) = {
            let inner = self.inner.lock().await;
            if inner.worker_id.is_none() {
                return Err(WorkerError::NotRegistered);
            }
            if inner.draining {
                return Err(WorkerError::Draining);
            }
            (inner.control.clone(), inner.draining)
        };

        if draining {
            return Err(WorkerError::Draining);
        }

        // Try to get a job assignment.
        let assignment = control.assign_next().await;
        let (job, _worker_id) = match assignment {
            Ok(pair) => pair,
            Err(polkagent_cloud_control::ControlError::NoWorkersAvailable) => return Ok(None),
            Err(e) => return Err(WorkerError::Control(e)),
        };

        // Track active jobs.
        {
            let mut inner = self.inner.lock().await;
            inner.active_jobs += 1;
        }

        let job_id = job.id.clone();

        // Execute the job.
        let result = executor.execute(&job).await;

        // Update counters and report to control plane.
        {
            let mut inner = self.inner.lock().await;
            inner.active_jobs = inner.active_jobs.saturating_sub(1);

            match &result {
                Ok(_) => {
                    inner.completed_jobs += 1;
                    let _ = inner.control.complete_job(&job_id).await;
                }
                Err(reason) => {
                    inner.failed_jobs += 1;
                    let _ = inner.control.fail_job(&job_id, reason.as_str()).await;
                }
            }

            // If draining and no active jobs remain, notify waiters.
            if inner.draining && inner.active_jobs == 0 {
                self.drain_notify.notify_waiters();
            }
        }

        match result {
            Ok(desc) => Ok(Some(desc)),
            Err(reason) => Err(WorkerError::Execution(reason)),
        }
    }

    /// Initiate graceful drain: stop accepting new jobs but finish in-flight
    /// work.
    pub async fn drain(&self) -> Result<()> {
        let mut inner = self.inner.lock().await;
        let wid = inner.worker_id.ok_or(WorkerError::NotRegistered)?;
        inner.draining = true;

        // Tell control plane we are draining so it stops assigning us work.
        if let Ok(mut record) = inner.control.get_worker(wid).await {
            record.status = WorkerStatus::Draining;
            // Re-register with updated status.
            let _ = inner.control.register_worker(record).await;
        }

        tracing::info!(worker_id = %wid, "worker entering drain mode");

        // If no active jobs, signal immediately.
        if inner.active_jobs == 0 {
            self.drain_notify.notify_waiters();
        }

        Ok(())
    }

    /// Wait until all in-flight jobs have completed (only meaningful after
    /// calling [`drain`](Self::drain)).
    pub async fn wait_for_drain(&self) {
        let is_idle = {
            let inner = self.inner.lock().await;
            inner.active_jobs == 0
        };
        if !is_idle {
            self.drain_notify.notified().await;
        }
    }

    /// Returns `true` if the worker is in drain mode.
    pub async fn is_draining(&self) -> bool {
        let inner = self.inner.lock().await;
        inner.draining
    }

    /// Get a snapshot of the worker's current state.
    pub async fn snapshot(&self) -> Result<WorkerSnapshot> {
        let inner = self.inner.lock().await;
        let wid = inner.worker_id.ok_or(WorkerError::NotRegistered)?;
        Ok(WorkerSnapshot {
            worker_id: wid,
            name: inner.config.name.clone(),
            draining: inner.draining,
            active_jobs: inner.active_jobs,
            completed_jobs: inner.completed_jobs,
            failed_jobs: inner.failed_jobs,
            started_at: inner.started_at,
            last_heartbeat: inner.last_heartbeat,
        })
    }

    /// Deregister this worker from the control plane.
    pub async fn deregister(&self) -> Result<()> {
        let mut inner = self.inner.lock().await;
        let wid = inner.worker_id.ok_or(WorkerError::NotRegistered)?;
        inner.control.deregister_worker(wid).await?;
        inner.worker_id = None;
        tracing::info!("cloud worker deregistered");
        Ok(())
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        reason = "worker tests unwrap controlled queue fixtures after exercising the expected lifecycle"
    )]

    use super::*;
    use polkagent_cloud_control::{ControlPlane, Job, JobStatus};
    use polkagent_core::RunId;

    /// A simple executor that always succeeds.
    struct OkExecutor;

    #[async_trait]
    impl JobExecutor for OkExecutor {
        async fn execute(&self, job: &Job) -> std::result::Result<String, String> {
            Ok(format!("done:{}", job.id))
        }
    }

    /// An executor that always fails.
    struct FailExecutor;

    #[async_trait]
    impl JobExecutor for FailExecutor {
        async fn execute(&self, _job: &Job) -> std::result::Result<String, String> {
            Err("boom".into())
        }
    }

    fn make_config() -> WorkerConfig {
        WorkerConfig {
            name: "test-worker".into(),
            capabilities: vec!["cpu".into()],
            max_concurrent_jobs: 4,
            heartbeat_interval: Duration::from_secs(5),
        }
    }

    fn make_job(id: &str) -> Job {
        Job::new(id, RunId::new(), format!("payload-{id}"))
    }

    // --- Registration -------------------------------------------------------

    #[tokio::test]
    async fn register_assigns_worker_id() {
        let cp = ControlPlane::with_defaults();
        let worker = CloudWorker::new(cp.clone(), make_config());
        let id = worker.register().await.ok().unwrap();
        let workers = cp.list_workers().await;
        assert_eq!(workers.len(), 1);
        assert_eq!(workers[0].id, id);
    }

    #[tokio::test]
    async fn deregister_removes_worker() {
        let cp = ControlPlane::with_defaults();
        let worker = CloudWorker::new(cp.clone(), make_config());
        worker.register().await.ok();
        worker.deregister().await.ok();
        let workers = cp.list_workers().await;
        assert!(workers.is_empty());
    }

    // --- Heartbeat ----------------------------------------------------------

    #[tokio::test]
    async fn heartbeat_before_register_fails() {
        let cp = ControlPlane::with_defaults();
        let worker = CloudWorker::new(cp, make_config());
        let result = worker.send_heartbeat().await;
        assert!(matches!(result, Err(WorkerError::NotRegistered)));
    }

    #[tokio::test]
    async fn heartbeat_updates_snapshot() {
        let cp = ControlPlane::with_defaults();
        let worker = CloudWorker::new(cp, make_config());
        worker.register().await.ok();
        worker.send_heartbeat().await.ok();
        let snap = worker.snapshot().await.ok().unwrap();
        assert!(snap.last_heartbeat.is_some());
    }

    // --- Job execution ------------------------------------------------------

    #[tokio::test]
    async fn poll_before_register_fails() {
        let cp = ControlPlane::with_defaults();
        let worker = CloudWorker::new(cp, make_config());
        let result = worker.poll_and_execute(&OkExecutor).await;
        assert!(matches!(result, Err(WorkerError::NotRegistered)));
    }

    #[tokio::test]
    async fn poll_with_no_jobs_returns_none() {
        let cp = ControlPlane::with_defaults();
        let worker = CloudWorker::new(cp, make_config());
        worker.register().await.ok();
        let result = worker.poll_and_execute(&OkExecutor).await;
        // No jobs queued — assignment fails with NoWorkersAvailable which is
        // mapped to Ok(None).
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn successful_job_increments_completed() {
        let cp = ControlPlane::with_defaults();
        let worker = CloudWorker::new(cp.clone(), make_config());
        worker.register().await.ok();
        cp.enqueue(make_job("j1")).await.ok();

        let result = worker.poll_and_execute(&OkExecutor).await;
        assert!(result.is_ok());
        assert_eq!(result.ok().unwrap(), Some("done:j1".into()));

        let snap = worker.snapshot().await.ok().unwrap();
        assert_eq!(snap.completed_jobs, 1);
        assert_eq!(snap.failed_jobs, 0);
        assert_eq!(snap.active_jobs, 0);

        let job = cp.get_job("j1").await.ok().unwrap();
        assert_eq!(job.status, JobStatus::Completed);
    }

    #[tokio::test]
    async fn failed_job_increments_failed_counter() {
        let cp = ControlPlane::with_defaults();
        let worker = CloudWorker::new(cp.clone(), make_config());
        worker.register().await.ok();
        cp.enqueue(make_job("j1")).await.ok();

        let result = worker.poll_and_execute(&FailExecutor).await;
        assert!(result.is_err());

        let snap = worker.snapshot().await.ok().unwrap();
        assert_eq!(snap.failed_jobs, 1);
        assert_eq!(snap.completed_jobs, 0);

        let job = cp.get_job("j1").await.ok().unwrap();
        assert!(matches!(job.status, JobStatus::Failed { .. }));
    }

    // --- Drain --------------------------------------------------------------

    #[tokio::test]
    async fn drain_before_register_fails() {
        let cp = ControlPlane::with_defaults();
        let worker = CloudWorker::new(cp, make_config());
        let result = worker.drain().await;
        assert!(matches!(result, Err(WorkerError::NotRegistered)));
    }

    #[tokio::test]
    async fn drain_rejects_new_jobs() {
        let cp = ControlPlane::with_defaults();
        let worker = CloudWorker::new(cp.clone(), make_config());
        worker.register().await.ok();
        worker.drain().await.ok();

        assert!(worker.is_draining().await);

        cp.enqueue(make_job("j1")).await.ok();
        let result = worker.poll_and_execute(&OkExecutor).await;
        assert!(matches!(result, Err(WorkerError::Draining)));
    }

    #[tokio::test]
    async fn drain_with_no_active_jobs_completes_immediately() {
        let cp = ControlPlane::with_defaults();
        let worker = CloudWorker::new(cp, make_config());
        worker.register().await.ok();
        worker.drain().await.ok();

        // wait_for_drain should return immediately.
        tokio::time::timeout(Duration::from_millis(100), worker.wait_for_drain())
            .await
            .ok()
            .unwrap();
    }

    // --- Snapshot ------------------------------------------------------------

    #[tokio::test]
    async fn snapshot_reflects_config() {
        let cp = ControlPlane::with_defaults();
        let config = WorkerConfig {
            name: "my-node".into(),
            ..make_config()
        };
        let worker = CloudWorker::new(cp, config);
        worker.register().await.ok();

        let snap = worker.snapshot().await.ok().unwrap();
        assert_eq!(snap.name, "my-node");
        assert!(!snap.draining);
        assert_eq!(snap.active_jobs, 0);
        assert_eq!(snap.completed_jobs, 0);
        assert_eq!(snap.failed_jobs, 0);
    }
}
