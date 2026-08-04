//! Phase 4 Acceptance Tests — Harness Session Resume
//!
//! Tests the session snapshot persistence and reload lifecycle:
//! create → serialize → deserialize → verify continuity.
//!
//! - SR-01: SessionSnapshot round-trips through JSON.
//! - SR-02: SessionId preserved across serialization.
//! - SR-03: Backend state map preserved across serialization.
//! - SR-04: Turn count and working directory preserved.
//! - SR-05: persist_session_state / load_session_state lifecycle.
//! - SR-06: load_session_state fails for non-existent session.
//! - SR-07: SessionSnapshot with empty backend state.
//! - SR-08: Multiple sessions serialize independently.
//! - SR-09: HarnessConfig round-trips through serde.
//! - SR-10: HarnessCapabilities round-trips through serde.

use std::collections::HashMap;
use std::path::PathBuf;

use chrono::Utc;

use polkagent_harness_trait::{
    HarnessCapabilities, HarnessConfig, HarnessId, SessionId, SessionSnapshot,
    load_session_state, persist_session_state, remove_session_state,
    SessionResumeMode, McpMode, ToolInjection, CancelMode,
};

// =========================================================================
// SR-01: SessionSnapshot round-trips through JSON
// =========================================================================

#[test]
fn sr_01_snapshot_json_round_trip() {
    let snapshot = SessionSnapshot {
        session_id: SessionId::new(),
        harness_id: HarnessId::new("claude-code"),
        process_pid: Some(12345),
        started_at: Utc::now(),
        working_directory: Some(PathBuf::from("/tmp/project")),
        turn_count: 5,
        backend_state: {
            let mut m = HashMap::new();
            m.insert("thread_id".into(), serde_json::json!("thread-abc"));
            m.insert("conversation_id".into(), serde_json::json!("conv-xyz"));
            m
        },
    };

    let json = serde_json::to_string(&snapshot).expect("serialize");
    let loaded: SessionSnapshot = serde_json::from_str(&json).expect("deserialize");

    assert_eq!(loaded.session_id, snapshot.session_id);
    assert_eq!(loaded.harness_id.as_str(), "claude-code");
    assert_eq!(loaded.process_pid, Some(12345));
    assert_eq!(loaded.turn_count, 5);
}

// =========================================================================
// SR-02: SessionId preserved across serialization
// =========================================================================

#[test]
fn sr_02_session_id_preserved() {
    let id = SessionId::new();
    let json = serde_json::to_string(&id).expect("serialize");
    let loaded: SessionId = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(id, loaded);

    let uuid = id.as_uuid();
    let from_uuid = SessionId::from_uuid(uuid);
    assert_eq!(id, from_uuid);
}

// =========================================================================
// SR-03: Backend state map preserved across serialization
// =========================================================================

#[test]
fn sr_03_backend_state_preserved() {
    let mut backend = HashMap::new();
    backend.insert("model".into(), serde_json::json!("claude-opus-4"));
    backend.insert("max_tokens".into(), serde_json::json!(4096));
    backend.insert("tools".into(), serde_json::json!(["bash", "read", "write"]));

    let snapshot = SessionSnapshot {
        session_id: SessionId::new(),
        harness_id: HarnessId::new("test"),
        process_pid: None,
        started_at: Utc::now(),
        working_directory: None,
        turn_count: 0,
        backend_state: backend.clone(),
    };

    let json = serde_json::to_string_pretty(&snapshot).expect("serialize");
    let loaded: SessionSnapshot = serde_json::from_str(&json).expect("deserialize");

    assert_eq!(loaded.backend_state.len(), 3);
    assert_eq!(
        loaded.backend_state.get("model"),
        Some(&serde_json::json!("claude-opus-4"))
    );
    assert_eq!(
        loaded.backend_state.get("max_tokens"),
        Some(&serde_json::json!(4096))
    );
}

// =========================================================================
// SR-04: Turn count and working directory preserved
// =========================================================================

#[test]
fn sr_04_turn_count_and_workdir_preserved() {
    let snapshot = SessionSnapshot {
        session_id: SessionId::new(),
        harness_id: HarnessId::new("codex"),
        process_pid: Some(99999),
        started_at: Utc::now(),
        working_directory: Some(PathBuf::from("/home/user/my-project")),
        turn_count: 42,
        backend_state: HashMap::new(),
    };

    let json = serde_json::to_string(&snapshot).expect("serialize");
    let loaded: SessionSnapshot = serde_json::from_str(&json).expect("deserialize");

    assert_eq!(loaded.turn_count, 42);
    assert_eq!(
        loaded.working_directory,
        Some(PathBuf::from("/home/user/my-project"))
    );
    assert_eq!(loaded.process_pid, Some(99999));
}

// =========================================================================
// SR-05: persist + load lifecycle via filesystem
// =========================================================================

#[test]
fn sr_05_persist_load_lifecycle() {
    let snapshot = SessionSnapshot {
        session_id: SessionId::new(),
        harness_id: HarnessId::new("test-harness"),
        process_pid: Some(1234),
        started_at: Utc::now(),
        working_directory: Some(PathBuf::from("/tmp/test")),
        turn_count: 3,
        backend_state: {
            let mut m = HashMap::new();
            m.insert("key".into(), serde_json::json!("value"));
            m
        },
    };

    persist_session_state(&snapshot).expect("persist");
    let loaded = load_session_state(snapshot.session_id).expect("load");

    assert_eq!(loaded.session_id, snapshot.session_id);
    assert_eq!(loaded.harness_id.as_str(), "test-harness");
    assert_eq!(loaded.turn_count, 3);
    assert_eq!(loaded.backend_state.get("key"), Some(&serde_json::json!("value")));

    remove_session_state(snapshot.session_id).expect("cleanup");
}

// =========================================================================
// SR-06: load_session_state fails for non-existent session
// =========================================================================

#[test]
fn sr_06_load_nonexistent_fails() {
    let fake_id = SessionId::new();
    let result = load_session_state(fake_id);
    assert!(result.is_err());
}

// =========================================================================
// SR-07: SessionSnapshot with empty backend state
// =========================================================================

#[test]
fn sr_07_empty_backend_state() {
    let snapshot = SessionSnapshot {
        session_id: SessionId::new(),
        harness_id: HarnessId::new("minimal"),
        process_pid: None,
        started_at: Utc::now(),
        working_directory: None,
        turn_count: 0,
        backend_state: HashMap::new(),
    };

    let json = serde_json::to_string(&snapshot).expect("serialize");
    let loaded: SessionSnapshot = serde_json::from_str(&json).expect("deserialize");
    assert!(loaded.backend_state.is_empty());
    assert!(loaded.process_pid.is_none());
    assert!(loaded.working_directory.is_none());
}

// =========================================================================
// SR-08: Multiple sessions serialize independently
// =========================================================================

#[test]
fn sr_08_multiple_sessions_independent() {
    let s1 = SessionSnapshot {
        session_id: SessionId::new(),
        harness_id: HarnessId::new("claude-code"),
        process_pid: Some(1001),
        started_at: Utc::now(),
        working_directory: Some(PathBuf::from("/project-a")),
        turn_count: 10,
        backend_state: {
            let mut m = HashMap::new();
            m.insert("session".into(), serde_json::json!("alpha"));
            m
        },
    };

    let s2 = SessionSnapshot {
        session_id: SessionId::new(),
        harness_id: HarnessId::new("codex"),
        process_pid: Some(2002),
        started_at: Utc::now(),
        working_directory: Some(PathBuf::from("/project-b")),
        turn_count: 20,
        backend_state: {
            let mut m = HashMap::new();
            m.insert("session".into(), serde_json::json!("beta"));
            m
        },
    };

    assert_ne!(s1.session_id, s2.session_id);

    let j1 = serde_json::to_string(&s1).expect("serialize s1");
    let j2 = serde_json::to_string(&s2).expect("serialize s2");

    let l1: SessionSnapshot = serde_json::from_str(&j1).expect("deser s1");
    let l2: SessionSnapshot = serde_json::from_str(&j2).expect("deser s2");

    assert_eq!(l1.harness_id.as_str(), "claude-code");
    assert_eq!(l2.harness_id.as_str(), "codex");
    assert_eq!(l1.turn_count, 10);
    assert_eq!(l2.turn_count, 20);
    assert_ne!(l1.session_id, l2.session_id);
}

// =========================================================================
// SR-09: HarnessConfig round-trips through serde
// =========================================================================

#[test]
fn sr_09_harness_config_serde() {
    let config = HarnessConfig::new("claude-code");

    let json = serde_json::to_string(&config).expect("serialize");
    let loaded: HarnessConfig = serde_json::from_str(&json).expect("deserialize");

    assert_eq!(loaded.id.as_str(), "claude-code");
    assert!(loaded.executable_path.is_none());
}

// =========================================================================
// SR-10: HarnessCapabilities round-trips through serde
// =========================================================================

#[test]
fn sr_10_harness_capabilities_serde() {
    let caps = HarnessCapabilities {
        supports_streaming: true,
        supports_tools: true,
        supports_sessions: true,
        max_context_tokens: 200_000,
        models: vec!["claude-opus-4".into(), "claude-sonnet-4".into()],
        transport: None,
        model_override: None,
        session_resume: SessionResumeMode::ById,
        mcp_passthrough: McpMode::Configurable,
        tool_injection: ToolInjection::McpConfig,
        cancel: CancelMode::Api,
        multiplex_safe: false,
    };

    let json = serde_json::to_string(&caps).expect("serialize");
    let loaded: HarnessCapabilities = serde_json::from_str(&json).expect("deserialize");

    assert!(loaded.supports_streaming);
    assert!(loaded.supports_sessions);
    assert_eq!(loaded.max_context_tokens, 200_000);
    assert_eq!(loaded.models.len(), 2);
    assert_eq!(loaded.session_resume, SessionResumeMode::ById);
    assert_eq!(loaded.mcp_passthrough, McpMode::Configurable);
}
