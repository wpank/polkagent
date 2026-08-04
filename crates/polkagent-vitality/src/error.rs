//! Error types for the vitality crate.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum VitalityError {
    #[error("affect dimension out of range: {dimension} = {value}")]
    OutOfRange { dimension: String, value: f64 },

    #[error("no run snapshots recorded yet")]
    NoData,
}
