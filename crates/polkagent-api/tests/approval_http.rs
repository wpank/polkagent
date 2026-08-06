//! Authenticated/read-only coverage for the scoped APR-05 HTTP adapter.

#![allow(
    clippy::expect_used,
    reason = "HTTP fixtures fail at explicit authentication and routing boundaries"
)]

use std::{path::Path, sync::Arc};

use async_trait::async_trait;
use axum::http::StatusCode;
use axum_test::TestServer;
use polkagent_api::{ApiServer, AppState, InMemoryAgentStore, InMemoryRunManager};
use polkagent_config::Config;
use polkagent_core::{ApprovalId, ConversationId, EffectId, RunId};
use polkagent_event::EventBus;
use polkagent_interaction::{
    ApprovalStatus, ApprovalView, BoxInteractionEventStream, ConfigUpdate,
    CreateInteractionRequest, InteractionConfig, InteractionError, InteractionErrorCode,
    InteractionService, InteractionSummary, InteractionTranscriptTurn, InteractionTurnId,
    ListInteractionsRequest, PromptRequest, StartedTurn, SubscriptionRequest, TranscriptRequest,
    TurnSummary,
};
use polkagent_store_sqlite::{migrations, SqlitePool};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

struct ApprovalHttpService {
    conversation_id: ConversationId,
    approval: Mutex<ApprovalView>,
}

impl ApprovalHttpService {
    fn unsupported() -> InteractionError {
        InteractionError::new(
            InteractionErrorCode::Unsupported,
            "not used by approval HTTP fixture",
        )
    }
}

#[async_trait]
impl InteractionService for ApprovalHttpService {
    async fn new_interaction(
        &self,
        _request: CreateInteractionRequest,
    ) -> Result<InteractionSummary, InteractionError> {
        Err(Self::unsupported())
    }

    async fn list_interactions(
        &self,
        _request: ListInteractionsRequest,
    ) -> Result<Vec<InteractionSummary>, InteractionError> {
        Err(Self::unsupported())
    }

    async fn load_interaction(
        &self,
        _conversation_id: ConversationId,
    ) -> Result<InteractionSummary, InteractionError> {
        Err(Self::unsupported())
    }

    async fn verify_interaction_origin(
        &self,
        _conversation_id: ConversationId,
        _working_directory: &Path,
    ) -> Result<(), InteractionError> {
        Err(Self::unsupported())
    }

    async fn list_turns(
        &self,
        _conversation_id: ConversationId,
    ) -> Result<Vec<TurnSummary>, InteractionError> {
        Err(Self::unsupported())
    }

    async fn load_transcript(
        &self,
        _request: TranscriptRequest,
    ) -> Result<Vec<InteractionTranscriptTurn>, InteractionError> {
        Err(Self::unsupported())
    }

    async fn delete_interaction(
        &self,
        _conversation_id: ConversationId,
    ) -> Result<(), InteractionError> {
        Err(Self::unsupported())
    }

    async fn prompt(&self, _request: PromptRequest) -> Result<StartedTurn, InteractionError> {
        Err(Self::unsupported())
    }

    async fn cancel_turn(&self, _turn_id: InteractionTurnId) -> Result<(), InteractionError> {
        Err(Self::unsupported())
    }

    async fn set_config_option(
        &self,
        _conversation_id: ConversationId,
        _update: ConfigUpdate,
    ) -> Result<InteractionConfig, InteractionError> {
        Err(Self::unsupported())
    }

    async fn list_pending_approvals(
        &self,
        conversation_id: ConversationId,
    ) -> Result<Vec<ApprovalView>, InteractionError> {
        if conversation_id != self.conversation_id {
            return Err(InteractionError::new(
                InteractionErrorCode::PermissionDenied,
                "approval scope does not match",
            ));
        }
        let approval = self.approval.lock().await.clone();
        Ok((approval.status == ApprovalStatus::Pending)
            .then_some(approval)
            .into_iter()
            .collect())
    }

    async fn approve(
        &self,
        conversation_id: ConversationId,
        approval_id: ApprovalId,
    ) -> Result<ApprovalView, InteractionError> {
        if conversation_id != self.conversation_id {
            return Err(InteractionError::new(
                InteractionErrorCode::PermissionDenied,
                "approval scope does not match",
            ));
        }
        let mut approval = self.approval.lock().await;
        if approval.approval_id != approval_id {
            return Err(InteractionError::new(
                InteractionErrorCode::NotFound,
                "approval not found",
            ));
        }
        match approval.status {
            ApprovalStatus::Pending | ApprovalStatus::Approved => {
                approval.status = ApprovalStatus::Approved;
                Ok(approval.clone())
            }
            _ => Err(InteractionError::new(
                InteractionErrorCode::Conflict,
                "different decision already won",
            )),
        }
    }

    async fn deny(
        &self,
        conversation_id: ConversationId,
        approval_id: ApprovalId,
        _reason: Option<String>,
    ) -> Result<ApprovalView, InteractionError> {
        if conversation_id != self.conversation_id {
            return Err(InteractionError::new(
                InteractionErrorCode::PermissionDenied,
                "approval scope does not match",
            ));
        }
        let mut approval = self.approval.lock().await;
        if approval.approval_id != approval_id {
            return Err(InteractionError::new(
                InteractionErrorCode::NotFound,
                "approval not found",
            ));
        }
        match approval.status {
            ApprovalStatus::Pending | ApprovalStatus::Denied => {
                approval.status = ApprovalStatus::Denied;
                Ok(approval.clone())
            }
            _ => Err(InteractionError::new(
                InteractionErrorCode::Conflict,
                "different decision already won",
            )),
        }
    }

    async fn subscribe(
        &self,
        _request: SubscriptionRequest,
    ) -> Result<BoxInteractionEventStream, InteractionError> {
        Err(Self::unsupported())
    }
}

fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn make_server(
    read_only: bool,
    compose_approvals: bool,
) -> (TestServer, ConversationId, ApprovalId) {
    let conversation_id = ConversationId::new();
    let approval_id = ApprovalId::new();
    let service: Arc<dyn InteractionService> = Arc::new(ApprovalHttpService {
        conversation_id,
        approval: Mutex::new(ApprovalView {
            approval_id,
            effect_id: EffectId::new(),
            run_id: RunId::new(),
            tool_call_id: None,
            title: "Write notes.txt".to_owned(),
            description: "Write one workspace file".to_owned(),
            status: ApprovalStatus::Pending,
            policy_reason: Some("write grant requires approval".to_owned()),
            expires_at: Some(chrono::Utc::now() + chrono::Duration::minutes(5)),
        }),
    });
    let pool = SqlitePool::open_in_memory().expect("open API store");
    migrations::migrate(&pool.writer()).expect("migrate API store");
    let mut config = Config::default();
    config.auth.enabled = true;
    config.auth.api_keys = vec![hash_token("operator-token")];
    config.api.read_only = read_only;
    let mut state = AppState::new(
        config,
        Arc::new(InMemoryAgentStore::new()),
        Arc::new(InMemoryRunManager::new()),
        Arc::new(pool),
        EventBus::with_default_capacity(),
    );
    if compose_approvals {
        state = state.with_authenticated_approval_service(service);
    }
    (
        TestServer::new(ApiServer::from_state(state).into_router()),
        conversation_id,
        approval_id,
    )
}

#[tokio::test]
async fn approval_routes_require_auth_and_explicit_surface_composition() {
    let (server, conversation_id, _approval_id) = make_server(false, true);
    let path = format!("/api/v1alpha1/interactions/{conversation_id}/approvals");
    assert_eq!(
        server.get(&path).await.status_code(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        server
            .get(&path)
            .add_header("authorization", "Bearer wrong-token")
            .await
            .status_code(),
        StatusCode::UNAUTHORIZED
    );
    let response = server
        .get(&path)
        .add_header("authorization", "Bearer operator-token")
        .await;
    assert_eq!(response.status_code(), StatusCode::OK);
    let body: serde_json::Value = response.json();
    assert_eq!(body["data"][0]["status"], "pending");

    let (uncomposed, conversation_id, _) = make_server(false, false);
    let response = uncomposed
        .get(&format!(
            "/api/v1alpha1/interactions/{conversation_id}/approvals"
        ))
        .add_header("authorization", "Bearer operator-token")
        .await;
    assert_eq!(response.status_code(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn approval_decisions_use_exact_identity_and_read_only_guard() {
    let (server, conversation_id, approval_id) = make_server(false, true);
    let path =
        format!("/api/v1alpha1/interactions/{conversation_id}/approvals/{approval_id}/approve");
    let response = server
        .post(&path)
        .add_header("x-api-key", "operator-token")
        .await;
    assert_eq!(response.status_code(), StatusCode::OK);
    let body: serde_json::Value = response.json();
    assert_eq!(body["approval"]["approval_id"], approval_id.to_string());
    assert_eq!(body["approval"]["status"], "approved");
    assert_eq!(
        server
            .post(&path)
            .add_header("x-api-key", "operator-token")
            .await
            .status_code(),
        StatusCode::OK
    );

    let wrong_scope = server
        .post(&format!(
            "/api/v1alpha1/interactions/{}/approvals/{approval_id}/approve",
            ConversationId::new()
        ))
        .add_header("x-api-key", "operator-token")
        .await;
    assert_eq!(wrong_scope.status_code(), StatusCode::FORBIDDEN);

    let (read_only, conversation_id, approval_id) = make_server(true, true);
    let response = read_only
        .post(&format!(
            "/api/v1alpha1/interactions/{conversation_id}/approvals/{approval_id}/deny"
        ))
        .add_header("x-api-key", "operator-token")
        .json(&serde_json::json!({"reason":"no"}))
        .await;
    assert_eq!(response.status_code(), StatusCode::METHOD_NOT_ALLOWED);
}

#[tokio::test]
async fn denial_reason_limit_counts_unicode_characters() {
    let (server, conversation_id, approval_id) = make_server(false, true);
    let path = format!("/api/v1alpha1/interactions/{conversation_id}/approvals/{approval_id}/deny");
    let response = server
        .post(&path)
        .add_header("x-api-key", "operator-token")
        .json(&serde_json::json!({"reason":"é".repeat(4_096)}))
        .await;
    assert_eq!(response.status_code(), StatusCode::OK);

    let (server, conversation_id, approval_id) = make_server(false, true);
    let response = server
        .post(&format!(
            "/api/v1alpha1/interactions/{conversation_id}/approvals/{approval_id}/deny"
        ))
        .add_header("x-api-key", "operator-token")
        .json(&serde_json::json!({"reason":"é".repeat(4_097)}))
        .await;
    assert_eq!(response.status_code(), StatusCode::UNPROCESSABLE_ENTITY);
}
