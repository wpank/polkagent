use chrono::{DateTime, Utc};
use polkagent_core::RunId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeteredEventKind {
    RunCompleted,
    TurnCompleted,
}

impl std::fmt::Display for MeteredEventKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RunCompleted => write!(f, "run_completed"),
            Self::TurnCompleted => write!(f, "turn_completed"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MeteredEvent {
    pub kind: MeteredEventKind,
    pub run_id: RunId,
    pub provider: String,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
    pub timestamp: DateTime<Utc>,
}

pub struct MeterEmitter {
    events: std::sync::Mutex<Vec<MeteredEvent>>,
}

impl MeterEmitter {
    #[must_use]
    pub fn new() -> Self {
        Self {
            events: std::sync::Mutex::new(Vec::new()),
        }
    }

    pub fn emit(&self, event: MeteredEvent) {
        tracing::info!(
            run_id = %event.run_id,
            kind = %event.kind,
            provider = %event.provider,
            model = %event.model,
            input_tokens = event.input_tokens,
            output_tokens = event.output_tokens,
            cost_usd = event.cost_usd,
            "metered billing event"
        );

        if let Ok(mut events) = self.events.lock() {
            events.push(event);
        }
    }

    pub fn emit_run_completed(
        &self,
        run_id: RunId,
        provider: &str,
        model: &str,
        input_tokens: u64,
        output_tokens: u64,
        cost_usd: f64,
    ) {
        self.emit(MeteredEvent {
            kind: MeteredEventKind::RunCompleted,
            run_id,
            provider: provider.to_owned(),
            model: model.to_owned(),
            input_tokens,
            output_tokens,
            cost_usd,
            timestamp: Utc::now(),
        });
    }

    pub fn emit_turn_completed(
        &self,
        run_id: RunId,
        provider: &str,
        model: &str,
        input_tokens: u64,
        output_tokens: u64,
        cost_usd: f64,
    ) {
        self.emit(MeteredEvent {
            kind: MeteredEventKind::TurnCompleted,
            run_id,
            provider: provider.to_owned(),
            model: model.to_owned(),
            input_tokens,
            output_tokens,
            cost_usd,
            timestamp: Utc::now(),
        });
    }

    pub fn drain(&self) -> Vec<MeteredEvent> {
        if let Ok(mut events) = self.events.lock() {
            std::mem::take(&mut *events)
        } else {
            Vec::new()
        }
    }

    #[must_use]
    pub fn pending_count(&self) -> usize {
        self.events.lock().map(|e| e.len()).unwrap_or(0)
    }
}

impl Default for MeterEmitter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_run_id() -> RunId {
        RunId::new()
    }

    #[test]
    fn emit_and_drain() {
        let emitter = MeterEmitter::new();
        let run_id = test_run_id();

        emitter.emit_run_completed(run_id, "anthropic", "claude-sonnet-4", 1000, 500, 0.0105);
        assert_eq!(emitter.pending_count(), 1);

        let events = emitter.drain();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, MeteredEventKind::RunCompleted);
        assert_eq!(events[0].provider, "anthropic");
        assert_eq!(events[0].input_tokens, 1000);
        assert_eq!(events[0].output_tokens, 500);

        assert_eq!(emitter.pending_count(), 0);
    }

    #[test]
    fn emit_turn_completed() {
        let emitter = MeterEmitter::new();
        let run_id = test_run_id();

        emitter.emit_turn_completed(run_id, "openai", "gpt-4o", 2000, 800, 0.013);
        let events = emitter.drain();

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, MeteredEventKind::TurnCompleted);
        assert_eq!(events[0].model, "gpt-4o");
    }

    #[test]
    fn multiple_events() {
        let emitter = MeterEmitter::new();
        let run_id = test_run_id();

        emitter.emit_turn_completed(run_id, "anthropic", "claude-sonnet-4", 100, 50, 0.001);
        emitter.emit_turn_completed(run_id, "anthropic", "claude-sonnet-4", 200, 100, 0.002);
        emitter.emit_run_completed(run_id, "anthropic", "claude-sonnet-4", 300, 150, 0.003);

        assert_eq!(emitter.pending_count(), 3);
        let events = emitter.drain();
        assert_eq!(events.len(), 3);
        assert_eq!(events[2].kind, MeteredEventKind::RunCompleted);
    }

    #[test]
    fn drain_clears_events() {
        let emitter = MeterEmitter::new();
        emitter.emit_run_completed(test_run_id(), "local", "local", 0, 0, 0.0);

        let first = emitter.drain();
        assert_eq!(first.len(), 1);

        let second = emitter.drain();
        assert!(second.is_empty());
    }

    #[test]
    fn metered_event_kind_display() {
        assert_eq!(MeteredEventKind::RunCompleted.to_string(), "run_completed");
        assert_eq!(
            MeteredEventKind::TurnCompleted.to_string(),
            "turn_completed"
        );
    }

    #[test]
    fn metered_event_serde_round_trip() {
        let event = MeteredEvent {
            kind: MeteredEventKind::RunCompleted,
            run_id: test_run_id(),
            provider: "anthropic".into(),
            model: "claude-sonnet-4".into(),
            input_tokens: 1000,
            output_tokens: 500,
            cost_usd: 0.0105,
            timestamp: Utc::now(),
        };
        let json = serde_json::to_string(&event).expect("serialize");
        let back: MeteredEvent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(event.kind, back.kind);
        assert_eq!(event.provider, back.provider);
        assert_eq!(event.input_tokens, back.input_tokens);
    }
}
