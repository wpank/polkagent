//! Conversation management endpoints.
//!
//! | Method | Path | Handler |
//! |---|---|---|
//! | `POST` | `/conversations` | [`create_conversation`] |
//! | `GET` | `/conversations` | [`list_conversations`] |
//! | `GET` | `/conversations/:id` | [`get_conversation`] |
//! | `POST` | `/conversations/:id/messages` | [`add_message`] |
//! | `DELETE` | `/conversations/:id` | [`delete_conversation`] |
//!
//! All endpoints return 501 Not Implemented when no conversation store is
//! configured via [`AppState::conversation_store`].

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use chrono::Utc;
use tracing::instrument;
use uuid::Uuid;

use polkagent_conversation::types::{Conversation, Message, MessageContent, MessageRole};
use polkagent_core::ids::{AgentId, ConversationId};

use crate::{
    dto::{
        ConversationResponse, ConversationSummaryDto, CreateConversationRequest,
        CreateConversationResponse, CreateMessageRequest, CreateMessageResponse, CursorInfo,
        ListConversationsQuery, ListConversationsResponse, MessageDto, PageMeta, API_VERSION,
    },
    error::ApiError,
    state::AppState,
};

// ---------------------------------------------------------------------------
// POST /conversations
// ---------------------------------------------------------------------------

/// Create a new conversation for an agent.
///
/// Returns 501 Not Implemented when no conversation store is configured.
#[instrument(skip(state, body), fields(agent_id = %body.agent_id))]
pub async fn create_conversation(
    State(state): State<AppState>,
    Json(body): Json<CreateConversationRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let store = state
        .conversation_store
        .as_ref()
        .ok_or_else(|| ApiError::NotImplemented("conversation store not configured".to_owned()))?;

    let agent_id: AgentId = body
        .agent_id
        .parse()
        .map_err(|_| ApiError::ValidationError(format!("invalid agent_id: {}", body.agent_id)))?;

    let conv_id = ConversationId::new();
    let mut conv = Conversation::new(conv_id, agent_id);
    conv.title = body.title;
    if let Some(meta) = body.metadata {
        conv.metadata = meta;
    }

    let id = store
        .create(conv.clone())
        .await
        .map_err(|e| ApiError::InternalError(e.to_string()))?;

    Ok((
        StatusCode::CREATED,
        Json(CreateConversationResponse {
            version: API_VERSION.to_owned(),
            id: id.to_string(),
            agent_id: agent_id.to_string(),
            title: conv.title,
            created_at: conv.created_at,
        }),
    ))
}

// ---------------------------------------------------------------------------
// GET /conversations
// ---------------------------------------------------------------------------

/// List conversations with pagination.
///
/// Returns 501 Not Implemented when no conversation store is configured.
#[instrument(skip(state))]
pub async fn list_conversations(
    State(state): State<AppState>,
    Query(query): Query<ListConversationsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let store = state
        .conversation_store
        .as_ref()
        .ok_or_else(|| ApiError::NotImplemented("conversation store not configured".to_owned()))?;

    let agent_id: AgentId = query
        .agent_id
        .parse()
        .map_err(|_| ApiError::ValidationError(format!("invalid agent_id: {}", query.agent_id)))?;

    let limit = query.limit.unwrap_or(50).min(100) as usize;
    let offset = query.offset.unwrap_or(0) as usize;

    let summaries = store
        .list(agent_id, limit + 1, offset)
        .await
        .map_err(|e| ApiError::InternalError(e.to_string()))?;

    let has_more = summaries.len() > limit;
    let page: Vec<_> = summaries.into_iter().take(limit).collect();
    let next_cursor = if has_more {
        Some(format!("{}", offset + limit))
    } else {
        None
    };

    let data: Vec<ConversationSummaryDto> = page
        .into_iter()
        .map(|s| ConversationSummaryDto {
            id: s.id.to_string(),
            agent_id: s.agent_id.to_string(),
            title: s.title,
            message_count: s.message_count,
            last_message_at: s.last_message_at,
            created_at: s.created_at,
        })
        .collect();

    let page_size = data.len();

    Ok(Json(ListConversationsResponse {
        version: API_VERSION.to_owned(),
        data,
        cursor: CursorInfo {
            next: next_cursor,
            has_more,
        },
        meta: PageMeta { page_size },
    }))
}

// ---------------------------------------------------------------------------
// GET /conversations/:id
// ---------------------------------------------------------------------------

/// Get a specific conversation with its messages.
///
/// Returns 501 Not Implemented when no conversation store is configured.
/// Returns 404 if the conversation does not exist.
#[instrument(skip(state), fields(conversation_id = %id))]
pub async fn get_conversation(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let store = state
        .conversation_store
        .as_ref()
        .ok_or_else(|| ApiError::NotImplemented("conversation store not configured".to_owned()))?;

    let conv_id: ConversationId = id
        .parse()
        .map_err(|_| ApiError::ValidationError(format!("invalid conversation id: {id}")))?;

    let conv = store.get(conv_id).await.map_err(|e| {
        if e.to_string().contains("not found") {
            ApiError::NotFound(format!("conversation '{id}'"))
        } else {
            ApiError::InternalError(e.to_string())
        }
    })?;

    let messages = store
        .get_messages(conv_id, 100, 0)
        .await
        .map_err(|e| ApiError::InternalError(e.to_string()))?;

    let message_dtos: Vec<MessageDto> = messages.into_iter().map(MessageDto::from).collect();

    Ok(Json(ConversationResponse {
        version: API_VERSION.to_owned(),
        id: conv.id.to_string(),
        agent_id: conv.agent_id.to_string(),
        title: conv.title,
        message_count: conv.message_count,
        created_at: conv.created_at,
        updated_at: conv.updated_at,
        messages: message_dtos,
    }))
}

// ---------------------------------------------------------------------------
// POST /conversations/:id/messages
// ---------------------------------------------------------------------------

/// Add a message to a conversation.
///
/// Returns 501 Not Implemented when no conversation store is configured.
/// Returns 404 if the conversation does not exist.
#[instrument(skip(state, body), fields(conversation_id = %id))]
pub async fn add_message(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<CreateMessageRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let store = state
        .conversation_store
        .as_ref()
        .ok_or_else(|| ApiError::NotImplemented("conversation store not configured".to_owned()))?;

    let conv_id: ConversationId = id
        .parse()
        .map_err(|_| ApiError::ValidationError(format!("invalid conversation id: {id}")))?;

    // Verify the conversation exists (will return NotFound from the store if
    // it does not).
    store.get(conv_id).await.map_err(|e| {
        if e.to_string().contains("not found") {
            ApiError::NotFound(format!("conversation '{id}'"))
        } else {
            ApiError::InternalError(e.to_string())
        }
    })?;

    let role = match body.role.as_str() {
        "user" => MessageRole::User,
        "assistant" => MessageRole::Assistant,
        "system" => MessageRole::System,
        "tool" => MessageRole::Tool,
        other => {
            return Err(ApiError::ValidationError(format!(
                "invalid role: '{other}'; expected one of: user, assistant, system, tool"
            )));
        }
    };

    let content = MessageContent::Text {
        text: body.content.clone(),
    };

    let msg = Message {
        id: Uuid::now_v7(),
        conversation_id: conv_id,
        role,
        content,
        created_at: Utc::now(),
        token_count: None,
    };

    let msg_id = store
        .add_message(conv_id, msg.clone())
        .await
        .map_err(|e| ApiError::InternalError(e.to_string()))?;

    Ok((
        StatusCode::CREATED,
        Json(CreateMessageResponse {
            version: API_VERSION.to_owned(),
            id: msg_id.to_string(),
            conversation_id: conv_id.to_string(),
            role: body.role,
            content: body.content,
            created_at: msg.created_at,
        }),
    ))
}

// ---------------------------------------------------------------------------
// DELETE /conversations/:id
// ---------------------------------------------------------------------------

/// Delete a conversation and all its messages.
///
/// Returns 501 Not Implemented when no conversation store is configured.
/// Returns 404 if the conversation does not exist.
#[instrument(skip(state), fields(conversation_id = %id))]
pub async fn delete_conversation(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let store = state
        .conversation_store
        .as_ref()
        .ok_or_else(|| ApiError::NotImplemented("conversation store not configured".to_owned()))?;

    let conv_id: ConversationId = id
        .parse()
        .map_err(|_| ApiError::ValidationError(format!("invalid conversation id: {id}")))?;

    store.delete(conv_id).await.map_err(|e| {
        if e.to_string().contains("not found") {
            ApiError::NotFound(format!("conversation '{id}'"))
        } else {
            ApiError::InternalError(e.to_string())
        }
    })?;

    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use polkagent_api_test_helpers::*;
    use serde_json::json;

    /// Module providing shared test infrastructure.
    ///
    /// We define this as a child module so that the integration test helpers
    /// (AppState construction, etc.) are self-contained.
    mod polkagent_api_test_helpers {
        use super::*;
        use std::sync::Arc;

        use axum::{
            routing::{get, post},
            Router,
        };
        use axum_test::TestServer;
        use polkagent_config::Config;
        use polkagent_conversation::{ConversationStore, InMemoryConversationStore};
        use polkagent_event::EventBus;
        use polkagent_store_trait::EffectStore;

        use crate::run::InMemoryRunManager;
        use crate::state::{AppState, InMemoryAgentStore};

        /// Minimal no-op `EffectStore` used solely to satisfy `AppState`
        /// requirements in unit tests.
        pub struct NoopEffectStore;

        #[async_trait::async_trait]
        impl EffectStore for NoopEffectStore {
            async fn propose_intent(
                &self,
                _intent: polkagent_store_trait::StoredIntent,
            ) -> Result<(), polkagent_store_trait::StoreError> {
                Ok(())
            }

            async fn claim_intent(
                &self,
                _worker_id: polkagent_core::WorkerId,
                _lease_duration: std::time::Duration,
            ) -> Result<
                Option<polkagent_store_trait::StoredIntent>,
                polkagent_store_trait::StoreError,
            > {
                Ok(None)
            }

            async fn claim_intent_by_id(
                &self,
                intent_id: polkagent_core::EffectId,
                _worker_id: polkagent_core::WorkerId,
                _lease_duration: std::time::Duration,
            ) -> Result<polkagent_store_trait::StoredIntent, polkagent_store_trait::StoreError>
            {
                Err(polkagent_store_trait::StoreError::NotFound {
                    resource_type: "EffectIntent",
                    id: intent_id.to_string(),
                })
            }

            async fn release_claim(
                &self,
                _intent_id: polkagent_core::EffectId,
                _worker_id: polkagent_core::WorkerId,
            ) -> Result<(), polkagent_store_trait::StoreError> {
                Ok(())
            }

            async fn get_intent(
                &self,
                intent_id: polkagent_core::EffectId,
            ) -> Result<polkagent_store_trait::StoredIntent, polkagent_store_trait::StoreError>
            {
                Err(polkagent_store_trait::StoreError::NotFound {
                    resource_type: "EffectIntent",
                    id: intent_id.to_string(),
                })
            }

            async fn get_by_run(
                &self,
                _run_id: polkagent_core::RunId,
            ) -> Result<Vec<polkagent_store_trait::StoredIntent>, polkagent_store_trait::StoreError>
            {
                Ok(vec![])
            }

            async fn expired_leases(
                &self,
                _cutoff: polkagent_core::Timestamp,
            ) -> Result<Vec<polkagent_store_trait::StoredIntent>, polkagent_store_trait::StoreError>
            {
                Ok(vec![])
            }

            async fn record_attempt_start(
                &self,
                _attempt_id: polkagent_core::EffectAttemptId,
                _intent_id: polkagent_core::EffectId,
                _worker_id: polkagent_core::WorkerId,
                _payload: serde_json::Value,
            ) -> Result<(), polkagent_store_trait::StoreError> {
                Ok(())
            }

            async fn record_outcome(
                &self,
                _outcome: polkagent_store_trait::StoredOutcome,
            ) -> Result<(), polkagent_store_trait::StoreError> {
                Ok(())
            }

            async fn unconsumed_outcomes(
                &self,
                _run_id: polkagent_core::RunId,
            ) -> Result<Vec<polkagent_store_trait::StoredOutcome>, polkagent_store_trait::StoreError>
            {
                Ok(vec![])
            }

            async fn mark_outcomes_consumed(
                &self,
                _outcome_ids: &[polkagent_core::EffectOutcomeId],
            ) -> Result<(), polkagent_store_trait::StoreError> {
                Ok(())
            }

            async fn update_intent_state(
                &self,
                intent_id: polkagent_core::EffectId,
                _new_state: &str,
            ) -> Result<polkagent_store_trait::StoredIntent, polkagent_store_trait::StoreError>
            {
                Err(polkagent_store_trait::StoreError::NotFound {
                    resource_type: "EffectIntent",
                    id: intent_id.to_string(),
                })
            }
        }

        /// Build an `AppState` with a conversation store attached.
        pub fn state_with_conversation_store(store: Arc<dyn ConversationStore>) -> AppState {
            let agents = Arc::new(InMemoryAgentStore::new());
            let run_manager = Arc::new(InMemoryRunManager::new());
            let effect_store: Arc<dyn EffectStore> = Arc::new(NoopEffectStore);
            let event_bus = EventBus::with_default_capacity();
            AppState::new(
                Config::default(),
                agents,
                run_manager,
                effect_store,
                event_bus,
            )
            .with_conversation_store(store)
        }

        /// Build an `AppState` without a conversation store.
        pub fn state_without_conversation_store() -> AppState {
            let agents = Arc::new(InMemoryAgentStore::new());
            let run_manager = Arc::new(InMemoryRunManager::new());
            let effect_store: Arc<dyn EffectStore> = Arc::new(NoopEffectStore);
            let event_bus = EventBus::with_default_capacity();
            AppState::new(
                Config::default(),
                agents,
                run_manager,
                effect_store,
                event_bus,
            )
        }

        /// Build a `Router` with conversation routes, using the given
        /// `AppState`.
        pub fn conversation_router(state: AppState) -> Router {
            Router::new()
                .route(
                    "/api/v1alpha1/conversations",
                    post(create_conversation).get(list_conversations),
                )
                .route(
                    "/api/v1alpha1/conversations/{id}",
                    get(get_conversation).delete(delete_conversation),
                )
                .route(
                    "/api/v1alpha1/conversations/{id}/messages",
                    post(add_message),
                )
                .with_state(state)
        }

        /// Build a `TestServer` with an `InMemoryConversationStore`.
        pub fn test_server_with_conv_store() -> (TestServer, Arc<InMemoryConversationStore>) {
            let conv_store = Arc::new(InMemoryConversationStore::new());
            let state =
                state_with_conversation_store(conv_store.clone() as Arc<dyn ConversationStore>);
            let server = TestServer::new(conversation_router(state));
            (server, conv_store)
        }

        /// Build a `TestServer` without a conversation store configured.
        pub fn test_server_no_conv_store() -> TestServer {
            let state = state_without_conversation_store();
            TestServer::new(conversation_router(state))
        }
    }

    // -----------------------------------------------------------------------
    // Test 1: create conversation
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_create_conversation() {
        let (server, _store) = test_server_with_conv_store();
        let agent_id = AgentId::new();

        let resp = server
            .post("/api/v1alpha1/conversations")
            .json(&json!({
                "agent_id": agent_id.to_string(),
                "title": "My first chat"
            }))
            .await;

        resp.assert_status(StatusCode::CREATED);
        let body: serde_json::Value = resp.json();
        assert_eq!(body["version"], "v1alpha1");
        assert!(body["id"].is_string());
        assert_eq!(body["agent_id"], agent_id.to_string());
        assert_eq!(body["title"], "My first chat");
        assert!(body["created_at"].is_string());
    }

    // -----------------------------------------------------------------------
    // Test 2: list conversations
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_list_conversations() {
        let (server, _store) = test_server_with_conv_store();
        let agent_id = AgentId::new();

        // Create two conversations.
        for title in &["Chat 1", "Chat 2"] {
            server
                .post("/api/v1alpha1/conversations")
                .json(&json!({
                    "agent_id": agent_id.to_string(),
                    "title": title,
                }))
                .await;
        }

        let resp = server
            .get(&format!(
                "/api/v1alpha1/conversations?agent_id={}",
                agent_id
            ))
            .await;

        resp.assert_status(StatusCode::OK);
        let body: serde_json::Value = resp.json();
        assert_eq!(body["version"], "v1alpha1");
        let data = body["data"].as_array().expect("data array");
        assert_eq!(data.len(), 2);
        assert!(!body["cursor"]["has_more"].as_bool().expect("has_more"));
    }

    // -----------------------------------------------------------------------
    // Test 3: get conversation by ID
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_get_conversation_by_id() {
        let (server, _store) = test_server_with_conv_store();
        let agent_id = AgentId::new();

        // Create a conversation.
        let create_resp = server
            .post("/api/v1alpha1/conversations")
            .json(&json!({
                "agent_id": agent_id.to_string(),
                "title": "Detailed chat"
            }))
            .await;
        let create_body: serde_json::Value = create_resp.json();
        let conv_id = create_body["id"].as_str().expect("id");

        // Get it.
        let resp = server
            .get(&format!("/api/v1alpha1/conversations/{conv_id}"))
            .await;

        resp.assert_status(StatusCode::OK);
        let body: serde_json::Value = resp.json();
        assert_eq!(body["id"], conv_id);
        assert_eq!(body["title"], "Detailed chat");
        assert_eq!(body["message_count"], 0);
        assert!(body["messages"].as_array().expect("messages").is_empty());
    }

    // -----------------------------------------------------------------------
    // Test 4: add message to conversation
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_add_message() {
        let (server, _store) = test_server_with_conv_store();
        let agent_id = AgentId::new();

        let create_resp = server
            .post("/api/v1alpha1/conversations")
            .json(&json!({
                "agent_id": agent_id.to_string(),
            }))
            .await;
        let conv_id = create_resp.json::<serde_json::Value>()["id"]
            .as_str()
            .expect("id")
            .to_owned();

        let resp = server
            .post(&format!("/api/v1alpha1/conversations/{conv_id}/messages"))
            .json(&json!({
                "role": "user",
                "content": "Hello, agent!"
            }))
            .await;

        resp.assert_status(StatusCode::CREATED);
        let body: serde_json::Value = resp.json();
        assert_eq!(body["version"], "v1alpha1");
        assert!(body["id"].is_string());
        assert_eq!(body["conversation_id"], conv_id);
        assert_eq!(body["role"], "user");
        assert_eq!(body["content"], "Hello, agent!");

        // Verify the message appears when we GET the conversation.
        let get_resp = server
            .get(&format!("/api/v1alpha1/conversations/{conv_id}"))
            .await;
        let get_body: serde_json::Value = get_resp.json();
        assert_eq!(get_body["message_count"], 1);
        let messages = get_body["messages"].as_array().expect("messages");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["role"], "user");
    }

    // -----------------------------------------------------------------------
    // Test 5: delete conversation
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_delete_conversation() {
        let (server, _store) = test_server_with_conv_store();
        let agent_id = AgentId::new();

        let create_resp = server
            .post("/api/v1alpha1/conversations")
            .json(&json!({
                "agent_id": agent_id.to_string(),
                "title": "To be deleted"
            }))
            .await;
        let conv_id = create_resp.json::<serde_json::Value>()["id"]
            .as_str()
            .expect("id")
            .to_owned();

        let resp = server
            .delete(&format!("/api/v1alpha1/conversations/{conv_id}"))
            .await;
        resp.assert_status(StatusCode::NO_CONTENT);

        // Confirm it's gone.
        let get_resp = server
            .get(&format!("/api/v1alpha1/conversations/{conv_id}"))
            .await;
        get_resp.assert_status(StatusCode::NOT_FOUND);
    }

    // -----------------------------------------------------------------------
    // Test 6: 404 for missing conversation
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_get_missing_conversation_returns_404() {
        let (server, _store) = test_server_with_conv_store();
        let fake_id = ConversationId::new();

        let resp = server
            .get(&format!("/api/v1alpha1/conversations/{fake_id}"))
            .await;
        resp.assert_status(StatusCode::NOT_FOUND);

        let body: serde_json::Value = resp.json();
        assert_eq!(body["error"]["code"], "NOT_FOUND");
    }

    // -----------------------------------------------------------------------
    // Test 7: 501 when conversation store not configured
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_501_when_store_not_configured() {
        let server = test_server_no_conv_store();
        let agent_id = AgentId::new();

        // POST /conversations
        let resp = server
            .post("/api/v1alpha1/conversations")
            .json(&json!({
                "agent_id": agent_id.to_string(),
            }))
            .await;
        resp.assert_status(StatusCode::NOT_IMPLEMENTED);

        let body: serde_json::Value = resp.json();
        assert_eq!(body["error"]["code"], "NOT_IMPLEMENTED");

        // GET /conversations
        let resp = server
            .get(&format!(
                "/api/v1alpha1/conversations?agent_id={}",
                agent_id
            ))
            .await;
        resp.assert_status(StatusCode::NOT_IMPLEMENTED);

        // DELETE also returns 501
        let fake_id = ConversationId::new();
        let resp = server
            .delete(&format!("/api/v1alpha1/conversations/{fake_id}"))
            .await;
        resp.assert_status(StatusCode::NOT_IMPLEMENTED);
    }

    // -----------------------------------------------------------------------
    // Test 8: pagination works
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_pagination() {
        let (server, _store) = test_server_with_conv_store();
        let agent_id = AgentId::new();

        // Create 5 conversations.
        for i in 0..5 {
            server
                .post("/api/v1alpha1/conversations")
                .json(&json!({
                    "agent_id": agent_id.to_string(),
                    "title": format!("Chat {i}"),
                }))
                .await;
        }

        // Request with limit=2.
        let resp = server
            .get(&format!(
                "/api/v1alpha1/conversations?agent_id={}&limit=2",
                agent_id
            ))
            .await;
        resp.assert_status(StatusCode::OK);
        let body: serde_json::Value = resp.json();
        let data = body["data"].as_array().expect("data");
        assert_eq!(data.len(), 2);
        assert!(body["cursor"]["has_more"].as_bool().expect("has_more"));
        assert!(body["cursor"]["next"].is_string());
        assert_eq!(body["meta"]["page_size"], 2);

        // Request second page with offset=2, limit=2.
        let resp = server
            .get(&format!(
                "/api/v1alpha1/conversations?agent_id={}&limit=2&offset=2",
                agent_id
            ))
            .await;
        let body: serde_json::Value = resp.json();
        let data = body["data"].as_array().expect("data");
        assert_eq!(data.len(), 2);
        assert!(body["cursor"]["has_more"].as_bool().expect("has_more"));

        // Last page with offset=4, limit=2.
        let resp = server
            .get(&format!(
                "/api/v1alpha1/conversations?agent_id={}&limit=2&offset=4",
                agent_id
            ))
            .await;
        let body: serde_json::Value = resp.json();
        let data = body["data"].as_array().expect("data");
        assert_eq!(data.len(), 1);
        assert!(!body["cursor"]["has_more"].as_bool().expect("has_more"));
    }

    // -----------------------------------------------------------------------
    // Test 9: add message to non-existent conversation returns 404
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_add_message_to_missing_conversation_returns_404() {
        let (server, _store) = test_server_with_conv_store();
        let fake_id = ConversationId::new();

        let resp = server
            .post(&format!("/api/v1alpha1/conversations/{fake_id}/messages"))
            .json(&json!({
                "role": "user",
                "content": "Hello?"
            }))
            .await;
        resp.assert_status(StatusCode::NOT_FOUND);
    }

    // -----------------------------------------------------------------------
    // Test 10: create conversation without title
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_create_conversation_without_title() {
        let (server, _store) = test_server_with_conv_store();
        let agent_id = AgentId::new();

        let resp = server
            .post("/api/v1alpha1/conversations")
            .json(&json!({
                "agent_id": agent_id.to_string(),
            }))
            .await;

        resp.assert_status(StatusCode::CREATED);
        let body: serde_json::Value = resp.json();
        assert!(body["title"].is_null());
    }

    // -----------------------------------------------------------------------
    // Test 11: invalid role returns 422
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_invalid_message_role_returns_422() {
        let (server, _store) = test_server_with_conv_store();
        let agent_id = AgentId::new();

        let create_resp = server
            .post("/api/v1alpha1/conversations")
            .json(&json!({ "agent_id": agent_id.to_string() }))
            .await;
        let conv_id = create_resp.json::<serde_json::Value>()["id"]
            .as_str()
            .expect("id")
            .to_owned();

        let resp = server
            .post(&format!("/api/v1alpha1/conversations/{conv_id}/messages"))
            .json(&json!({
                "role": "alien",
                "content": "Take me to your leader"
            }))
            .await;
        resp.assert_status(StatusCode::UNPROCESSABLE_ENTITY);
    }
}
