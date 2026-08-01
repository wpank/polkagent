//! Telemetry initialization -- tracing subscriber, OTLP export, and guard.
//!
//! Call [`init_telemetry`] with a [`TelemetryConfig`] at program start,
//! or use [`init_from_env`] to read configuration from environment variables.
//! Hold the returned [`TelemetryGuard`] for the lifetime of the program;
//! dropping it flushes any pending spans/events.

use serde::{Deserialize, Serialize};
use tracing_subscriber::fmt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

use opentelemetry::trace::TracerProvider as _;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::trace::TracerProvider;

// ---------------------------------------------------------------------------
// Config types
// ---------------------------------------------------------------------------

/// Log output format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LogFormat {
    /// Human-friendly, coloured output (default).
    #[default]
    Pretty,
    /// Machine-readable newline-delimited JSON.
    Json,
}

/// Configuration for the telemetry subsystem.
#[derive(Debug, Clone)]
pub struct TelemetryConfig {
    /// Log level filter string (e.g. `"info"`, `"polkagent=debug,warn"`).
    pub log_level: String,
    /// Whether to emit pretty or JSON-formatted log lines.
    pub log_format: LogFormat,
    /// If set, traces are exported to this OTLP/gRPC endpoint.
    pub otlp_endpoint: Option<String>,
    /// The `service.name` resource attribute for traces.
    pub service_name: String,
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self {
            log_level: "info".to_owned(),
            log_format: LogFormat::Pretty,
            otlp_endpoint: None,
            service_name: "polkagent".to_owned(),
        }
    }
}

// ---------------------------------------------------------------------------
// TelemetryGuard
// ---------------------------------------------------------------------------

/// RAII guard returned by [`init_telemetry`].
///
/// When dropped, any active OTLP tracer provider is shut down and pending
/// spans are flushed.
pub struct TelemetryGuard {
    provider: Option<TracerProvider>,
}

impl TelemetryGuard {
    /// Return a no-op guard that performs no shutdown on drop.
    ///
    /// Useful when telemetry initialisation fails non-fatally (e.g. a
    /// subscriber is already installed in the current process).
    pub fn no_op() -> Self {
        Self { provider: None }
    }
}

impl Drop for TelemetryGuard {
    fn drop(&mut self) {
        if let Some(provider) = self.provider.take() {
            if let Err(e) = provider.shutdown() {
                eprintln!("telemetry shutdown error: {e}");
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Initialisation
// ---------------------------------------------------------------------------

/// Build the OTLP tracer provider and return it with a tracer.
fn build_otel_provider(
    endpoint: &str,
    service_name: &str,
) -> Result<(TracerProvider, opentelemetry_sdk::trace::Tracer), Box<dyn std::error::Error + Send + Sync>>
{
    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .with_endpoint(endpoint)
        .build()?;

    let provider = TracerProvider::builder()
        .with_simple_exporter(exporter)
        .build();

    let tracer = provider.tracer(service_name.to_owned());
    Ok((provider, tracer))
}

/// Initialise the global tracing subscriber with the given configuration.
///
/// Returns a [`TelemetryGuard`] that **must** be held for the lifetime of the
/// program. Dropping it flushes pending OTLP spans.
///
/// # Errors
///
/// Returns an error if OTLP exporter setup fails.
pub fn init_telemetry(
    config: TelemetryConfig,
) -> Result<TelemetryGuard, Box<dyn std::error::Error + Send + Sync>> {
    let env_filter = EnvFilter::try_new(&config.log_level)
        .unwrap_or_else(|_| EnvFilter::new("info"));

    // Build each combination of (format x otlp) separately so that the
    // tracing-subscriber type-level layering is fully resolved at compile time.
    let provider = match (&config.log_format, &config.otlp_endpoint) {
        (LogFormat::Pretty, Some(endpoint)) => {
            let (provider, tracer) = build_otel_provider(endpoint, &config.service_name)?;
            let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);
            tracing_subscriber::registry()
                .with(env_filter)
                .with(otel_layer)
                .with(fmt::layer().pretty())
                .init();
            Some(provider)
        }
        (LogFormat::Json, Some(endpoint)) => {
            let (provider, tracer) = build_otel_provider(endpoint, &config.service_name)?;
            let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);
            tracing_subscriber::registry()
                .with(env_filter)
                .with(otel_layer)
                .with(fmt::layer().json())
                .init();
            Some(provider)
        }
        (LogFormat::Pretty, None) => {
            tracing_subscriber::registry()
                .with(env_filter)
                .with(fmt::layer().pretty())
                .init();
            None
        }
        (LogFormat::Json, None) => {
            tracing_subscriber::registry()
                .with(env_filter)
                .with(fmt::layer().json())
                .init();
            None
        }
    };

    Ok(TelemetryGuard { provider })
}

/// Initialise telemetry from environment variables.
///
/// | Variable | Default | Meaning |
/// |---|---|---|
/// | `POLKAGENT_LOG_LEVEL` | `info` | Tracing filter directive |
/// | `POLKAGENT_LOG_FORMAT` | `pretty` | `pretty` or `json` |
/// | `OTEL_EXPORTER_OTLP_ENDPOINT` | *(none)* | OTLP gRPC endpoint |
///
/// # Errors
///
/// Returns an error if OTLP exporter setup fails.
pub fn init_from_env() -> Result<TelemetryGuard, Box<dyn std::error::Error + Send + Sync>> {
    let log_level = std::env::var("POLKAGENT_LOG_LEVEL")
        .unwrap_or_else(|_| "info".to_owned());

    let log_format = match std::env::var("POLKAGENT_LOG_FORMAT")
        .unwrap_or_default()
        .to_lowercase()
        .as_str()
    {
        "json" => LogFormat::Json,
        _ => LogFormat::Pretty,
    };

    let otlp_endpoint = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").ok();

    init_telemetry(TelemetryConfig {
        log_level,
        log_format,
        otlp_endpoint,
        service_name: "polkagent".to_owned(),
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_pretty_info() {
        let cfg = TelemetryConfig::default();
        assert_eq!(cfg.log_level, "info");
        assert_eq!(cfg.log_format, LogFormat::Pretty);
        assert!(cfg.otlp_endpoint.is_none());
        assert_eq!(cfg.service_name, "polkagent");
    }

    #[test]
    fn log_format_serde_round_trip() {
        let cases = [
            (LogFormat::Pretty, r#""pretty""#),
            (LogFormat::Json, r#""json""#),
        ];
        for (variant, expected) in &cases {
            let json = serde_json::to_string(variant).expect("serialize");
            assert_eq!(&json, expected);
            let back: LogFormat = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(variant, &back);
        }
    }

    #[test]
    fn guard_drop_without_provider_does_not_panic() {
        let guard = TelemetryGuard::no_op();
        drop(guard);
    }

    #[test]
    fn guard_no_op_constructor() {
        let guard = TelemetryGuard::no_op();
        // Dropping a no-op guard must not panic.
        drop(guard);
    }
}
