//! Standard span constructors for Polkagent observability.
//!
//! Each function creates a [`tracing::Span`] with a consistent set of
//! attributes, making it easy to correlate traces across the system.

use polkagent_core::ids::{EffectId, RunId};
use tracing::Span;

/// Create a span for an entire run.
///
/// # Attributes
/// - `otel.name` = `"run"`
/// - `service.name` = `"polkagent"`
/// - `run.id` = the run's UUID
pub fn run_span(run_id: &RunId) -> Span {
    tracing::info_span!(
        "run",
        service.name = "polkagent",
        run.id = %run_id,
    )
}

/// Create a span for a single turn within a run.
///
/// # Attributes
/// - `otel.name` = `"turn"`
/// - `service.name` = `"polkagent"`
/// - `run.id` = the run's UUID
/// - `turn.seq` = zero-based turn number
pub fn turn_span(run_id: &RunId, turn_seq: u32) -> Span {
    tracing::info_span!(
        "turn",
        service.name = "polkagent",
        run.id = %run_id,
        turn.seq = turn_seq,
    )
}

/// Create a span for an effect execution.
///
/// # Attributes
/// - `otel.name` = `"effect"`
/// - `service.name` = `"polkagent"`
/// - `effect.id` = the effect's UUID
/// - `effect.kind` = the effect kind string
pub fn effect_span(effect_id: &EffectId, kind: &str) -> Span {
    tracing::info_span!(
        "effect",
        service.name = "polkagent",
        effect.id = %effect_id,
        effect.kind = kind,
    )
}

/// Create a span for a model inference request.
///
/// # Attributes
/// - `otel.name` = `"model_request"`
/// - `service.name` = `"polkagent"`
/// - `model.provider` = the provider name
/// - `model.name` = the model identifier
pub fn model_request_span(provider: &str, model: &str) -> Span {
    tracing::info_span!(
        "model_request",
        service.name = "polkagent",
        model.provider = provider,
        model.name = model,
    )
}

/// Create a span for a tool call.
///
/// # Attributes
/// - `otel.name` = `"tool_call"`
/// - `service.name` = `"polkagent"`
/// - `tool.name` = the tool's name
pub fn tool_call_span(tool_name: &str) -> Span {
    tracing::info_span!(
        "tool_call",
        service.name = "polkagent",
        tool.name = tool_name,
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use polkagent_core::ids::{EffectId, RunId};
    use tracing_subscriber::layer::SubscriberExt;

    /// Install a no-op subscriber for this test so spans are not disabled.
    fn with_subscriber<F: FnOnce()>(f: F) {
        let subscriber = tracing_subscriber::registry()
            .with(tracing_subscriber::fmt::layer().with_test_writer());
        tracing::subscriber::with_default(subscriber, f);
    }

    #[test]
    fn run_span_is_not_disabled() {
        with_subscriber(|| {
            let run_id = RunId::new();
            let span = run_span(&run_id);
            assert!(!span.is_disabled());
        });
    }

    #[test]
    fn turn_span_is_not_disabled() {
        with_subscriber(|| {
            let run_id = RunId::new();
            let span = turn_span(&run_id, 0);
            assert!(!span.is_disabled());
        });
    }

    #[test]
    fn effect_span_is_not_disabled() {
        with_subscriber(|| {
            let eid = EffectId::new();
            let span = effect_span(&eid, "transfer");
            assert!(!span.is_disabled());
        });
    }

    #[test]
    fn model_request_span_is_not_disabled() {
        with_subscriber(|| {
            let span = model_request_span("anthropic", "claude-3-opus");
            assert!(!span.is_disabled());
        });
    }

    #[test]
    fn tool_call_span_is_not_disabled() {
        with_subscriber(|| {
            let span = tool_call_span("read_file");
            assert!(!span.is_disabled());
        });
    }
}
