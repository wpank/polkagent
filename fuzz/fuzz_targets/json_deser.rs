#![no_main]

use libfuzzer_sys::fuzz_target;

// Import all the types we want to fuzz-deserialize.
use polkagent_core::{
    // Run types
    RunState,
    // Effect types (from polkagent-core)
    EffectKind,
    EffectIntentState,
    AttemptState,
    // Agent types
    AgentState,
    AgentSpec,
    DegradationStage,
    // Event types
    EventKind,
    Durability,
    EventCorrelation,
    RunEvent,
    // Config types
    AutonomyLevel,
    DataClassification,
    // Run
    Run,
    // Turn
    TokenUsage,
    // Effect
    EffectIntent,
    EffectAttempt,
    EffectOutcome,
    OutcomeStatus,
    RetryClass,
    IdempotencyKey,
    ExternalRef,
};

fuzz_target!(|data: &[u8]| {
    // Try deserializing random bytes as each type.
    // All of these must never panic — only return Ok or Err.

    // First try raw bytes as JSON.
    let _ = serde_json::from_slice::<RunState>(data);
    let _ = serde_json::from_slice::<EffectKind>(data);
    let _ = serde_json::from_slice::<EffectIntentState>(data);
    let _ = serde_json::from_slice::<AttemptState>(data);
    let _ = serde_json::from_slice::<AgentState>(data);
    let _ = serde_json::from_slice::<AgentSpec>(data);
    let _ = serde_json::from_slice::<DegradationStage>(data);
    let _ = serde_json::from_slice::<EventKind>(data);
    let _ = serde_json::from_slice::<Durability>(data);
    let _ = serde_json::from_slice::<EventCorrelation>(data);
    let _ = serde_json::from_slice::<RunEvent>(data);
    let _ = serde_json::from_slice::<AutonomyLevel>(data);
    let _ = serde_json::from_slice::<DataClassification>(data);
    let _ = serde_json::from_slice::<Run>(data);
    let _ = serde_json::from_slice::<TokenUsage>(data);
    let _ = serde_json::from_slice::<EffectIntent>(data);
    let _ = serde_json::from_slice::<EffectAttempt>(data);
    let _ = serde_json::from_slice::<EffectOutcome>(data);
    let _ = serde_json::from_slice::<OutcomeStatus>(data);
    let _ = serde_json::from_slice::<RetryClass>(data);
    let _ = serde_json::from_slice::<IdempotencyKey>(data);
    let _ = serde_json::from_slice::<ExternalRef>(data);

    // Also try interpreting as a UTF-8 string and parsing as JSON Value
    // first, then converting — catches different code paths.
    if let Ok(s) = std::str::from_utf8(data) {
        let _ = serde_json::from_str::<RunState>(s);
        let _ = serde_json::from_str::<EffectKind>(s);
        let _ = serde_json::from_str::<AgentState>(s);
        let _ = serde_json::from_str::<EventKind>(s);
        let _ = serde_json::from_str::<Run>(s);
    }
});
