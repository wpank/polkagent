//! `polkagent-telemetry` — OpenTelemetry tracing, structured logging,
//! secret redaction, and JSONL event recording for the Polkagent platform.
//!
//! # Module overview
//!
//! | Module | Contents |
//! |--------|----------|
//! | [`init`] | Telemetry initialization with optional OTLP exporter |
//! | [`redact`] | `Redacted<T>` newtype and pattern-based string redaction |
//! | [`spans`] | Standard span constructors for runs, turns, effects, etc. |
//! | [`metrics`] | Key metric recording via tracing events |
//! | [`jsonl`] | JSONL event file writer for durable run event recording |

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

pub mod init;
pub mod jsonl;
pub mod metrics;
pub mod prometheus;
pub mod redact;
pub mod spans;

// ---------------------------------------------------------------------------
// Convenience re-exports
// ---------------------------------------------------------------------------

pub use init::{init_from_env, init_telemetry, LogFormat, TelemetryConfig, TelemetryGuard};
pub use jsonl::JsonlWriter;
pub use metrics::MetricRecorder;
pub use prometheus::PrometheusRegistry;
pub use redact::{redact_string, Redacted};
pub use spans::{effect_span, model_request_span, run_span, tool_call_span, turn_span};
