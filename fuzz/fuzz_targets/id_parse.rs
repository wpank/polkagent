#![no_main]

use libfuzzer_sys::fuzz_target;
use polkagent_core::{
    AgentId, ApprovalId, ArtifactId, ConversationId, EffectAttemptId, EffectId, EffectOutcomeId,
    EventId, GrantId, PrincipalId, RunId, StepId, TurnId, WorkerId,
};

fuzz_target!(|data: &[u8]| {
    // Try parsing random bytes as a UTF-8 string, then as each ID type.
    // All of these use Uuid::parse_str under the hood and must never panic.
    if let Ok(s) = std::str::from_utf8(data) {
        let _ = s.parse::<AgentId>();
        let _ = s.parse::<RunId>();
        let _ = s.parse::<TurnId>();
        let _ = s.parse::<StepId>();
        let _ = s.parse::<EffectId>();
        let _ = s.parse::<ArtifactId>();
        let _ = s.parse::<EventId>();
        let _ = s.parse::<ConversationId>();
        let _ = s.parse::<PrincipalId>();
        let _ = s.parse::<EffectAttemptId>();
        let _ = s.parse::<EffectOutcomeId>();
        let _ = s.parse::<GrantId>();
        let _ = s.parse::<ApprovalId>();
        let _ = s.parse::<WorkerId>();
    }

    // Also try JSON deserialization of the ID types (serde(transparent)
    // over UUID, so e.g. `"not-a-uuid"` is valid JSON but should produce
    // a serde error, not a panic).
    let _ = serde_json::from_slice::<AgentId>(data);
    let _ = serde_json::from_slice::<RunId>(data);
    let _ = serde_json::from_slice::<TurnId>(data);
    let _ = serde_json::from_slice::<StepId>(data);
    let _ = serde_json::from_slice::<EffectId>(data);
    let _ = serde_json::from_slice::<ArtifactId>(data);
    let _ = serde_json::from_slice::<EventId>(data);
    let _ = serde_json::from_slice::<ConversationId>(data);
    let _ = serde_json::from_slice::<PrincipalId>(data);
    let _ = serde_json::from_slice::<EffectAttemptId>(data);
    let _ = serde_json::from_slice::<EffectOutcomeId>(data);
    let _ = serde_json::from_slice::<GrantId>(data);
    let _ = serde_json::from_slice::<ApprovalId>(data);
    let _ = serde_json::from_slice::<WorkerId>(data);

    // Try Display and round-trip on valid IDs. This exercises the FromStr
    // → Display → FromStr path.
    if let Ok(s) = std::str::from_utf8(data) {
        if let Ok(id) = s.parse::<RunId>() {
            let displayed = id.to_string();
            let reparsed: Result<RunId, _> = displayed.parse();
            // If we successfully parsed, the round-trip must succeed.
            assert!(reparsed.is_ok());
        }
    }
});
