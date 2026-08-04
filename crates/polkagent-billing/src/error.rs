use thiserror::Error;

#[derive(Debug, Error)]
pub enum BillingError {
    #[error("run not found: {run_id}")]
    RunNotFound { run_id: String },

    #[error("no cost data available for run {run_id}")]
    NoCostData { run_id: String },

    #[error("store error: {0}")]
    Store(String),

    #[error("csv export error: {0}")]
    Export(String),
}
