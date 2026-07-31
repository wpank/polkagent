#![no_main]

use libfuzzer_sys::fuzz_target;
use polkagent_api::dto::{
    AgentLifecycleResponse, AgentResponse, ArtifactResponse, ConfigSummary, CreateAgentRequest,
    CreateRunRequest, CursorInfo, ErrorDetail, ErrorResponse, HealthResponse, InstallSkillRequest,
    ListAgentsResponse, ListArtifactsResponse, ListEventsResponse, ListModelsResponse,
    ListPaymentReceiptsResponse, ListProvidersResponse, ListRunsResponse, ListSkillsResponse,
    ListToolsResponse, ListTurnsResponse, MemoryForgetRequest, MemoryForgetResponse,
    MemoryQueryRequest, MemoryQueryResponse, MemoryStatsResponse, ModelCapabilities,
    ModelResponse, PageMeta, PaymentBalanceResponse, PaymentUsageResponse, ProvenanceResponse,
    ProviderResponse, RunResponse, SkillResponse, SystemInfoResponse, ToolGrantsResponse,
    ToolResponse, UpdateSkillConfigRequest,
};

fuzz_target!(|data: &[u8]| {
    // FZ-09: Fuzz JSON parsing for all API DTOs.
    // None of these must ever panic — only return Ok or Err.

    // ---- Request DTOs ----
    let _ = serde_json::from_slice::<CreateAgentRequest>(data);
    let _ = serde_json::from_slice::<CreateRunRequest>(data);
    let _ = serde_json::from_slice::<MemoryQueryRequest>(data);
    let _ = serde_json::from_slice::<MemoryForgetRequest>(data);
    let _ = serde_json::from_slice::<InstallSkillRequest>(data);
    let _ = serde_json::from_slice::<UpdateSkillConfigRequest>(data);

    // ---- Response DTOs ----
    let _ = serde_json::from_slice::<AgentResponse>(data);
    let _ = serde_json::from_slice::<ListAgentsResponse>(data);
    let _ = serde_json::from_slice::<RunResponse>(data);
    let _ = serde_json::from_slice::<ListRunsResponse>(data);
    let _ = serde_json::from_slice::<HealthResponse>(data);
    let _ = serde_json::from_slice::<SystemInfoResponse>(data);
    let _ = serde_json::from_slice::<ConfigSummary>(data);
    let _ = serde_json::from_slice::<ErrorResponse>(data);
    let _ = serde_json::from_slice::<ErrorDetail>(data);
    let _ = serde_json::from_slice::<CursorInfo>(data);
    let _ = serde_json::from_slice::<PageMeta>(data);
    let _ = serde_json::from_slice::<ArtifactResponse>(data);
    let _ = serde_json::from_slice::<ListArtifactsResponse>(data);
    let _ = serde_json::from_slice::<ProvenanceResponse>(data);
    let _ = serde_json::from_slice::<ListTurnsResponse>(data);
    let _ = serde_json::from_slice::<ListEventsResponse>(data);
    let _ = serde_json::from_slice::<ProviderResponse>(data);
    let _ = serde_json::from_slice::<ListProvidersResponse>(data);
    let _ = serde_json::from_slice::<ModelResponse>(data);
    let _ = serde_json::from_slice::<ModelCapabilities>(data);
    let _ = serde_json::from_slice::<ListModelsResponse>(data);
    let _ = serde_json::from_slice::<SkillResponse>(data);
    let _ = serde_json::from_slice::<ListSkillsResponse>(data);
    let _ = serde_json::from_slice::<ToolResponse>(data);
    let _ = serde_json::from_slice::<ToolGrantsResponse>(data);
    let _ = serde_json::from_slice::<ListToolsResponse>(data);
    let _ = serde_json::from_slice::<MemoryQueryResponse>(data);
    let _ = serde_json::from_slice::<MemoryForgetResponse>(data);
    let _ = serde_json::from_slice::<MemoryStatsResponse>(data);
    let _ = serde_json::from_slice::<PaymentBalanceResponse>(data);
    let _ = serde_json::from_slice::<PaymentUsageResponse>(data);
    let _ = serde_json::from_slice::<ListPaymentReceiptsResponse>(data);
    let _ = serde_json::from_slice::<AgentLifecycleResponse>(data);

    // Also try UTF-8 string paths for the most common request types.
    if let Ok(s) = std::str::from_utf8(data) {
        let _ = serde_json::from_str::<CreateAgentRequest>(s);
        let _ = serde_json::from_str::<CreateRunRequest>(s);
        let _ = serde_json::from_str::<MemoryQueryRequest>(s);
    }

    // Round-trip: if CreateAgentRequest deserializes, re-serialization must
    // not panic.
    if let Ok(req) = serde_json::from_slice::<CreateAgentRequest>(data) {
        let serialized = serde_json::to_string(&req);
        assert!(
            serialized.is_ok(),
            "serialization of a successfully deserialized CreateAgentRequest must not fail"
        );
    }

    // Round-trip: same for CreateRunRequest.
    if let Ok(req) = serde_json::from_slice::<CreateRunRequest>(data) {
        let serialized = serde_json::to_string(&req);
        assert!(
            serialized.is_ok(),
            "serialization of a successfully deserialized CreateRunRequest must not fail"
        );
    }
});
