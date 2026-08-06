//! Black-box coverage for checkpoint-aware durable interaction `SSE`.

#![allow(
    clippy::expect_used,
    clippy::too_many_lines,
    reason = "integration fixtures fail at explicit HTTP and durability boundaries"
)]

use std::{
    collections::VecDeque,
    io::Write,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex as StdMutex,
    },
    time::Duration,
};

use async_trait::async_trait;
use axum::http::StatusCode;
use axum_test::TestServer;
use polkagent_api::{ApiServer, AppState, InMemoryAgentStore, InMemoryRunManager};
use polkagent_config::Config;
use polkagent_core::{AgentId, AgentSpec, ConversationId, RunId};
use polkagent_event::{EventBus, EventRecorder};
use polkagent_interaction::{
    BoxInteractionEventStream, ConfigUpdate, CreateInteractionRequest, InteractionConfig,
    InteractionError, InteractionErrorCode, InteractionEvent, InteractionEventEnvelope,
    InteractionEventStream, InteractionService, InteractionSummary, InteractionTurnId,
    ListInteractionsRequest, PromptRequest, StartedTurn, StreamError, SubscriptionRequest,
    TurnSummary,
};
use polkagent_runtime::DurableInteractionService;
use polkagent_service::AppService;
use polkagent_store_sqlite::{migrations, SqlitePool};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;
use tracing_subscriber::fmt::MakeWriter;

const PRIVATE_BACKEND_MESSAGE: &str = "SENTINEL_BACKEND_MESSAGE_MUST_NOT_APPEAR";

struct LiveServer {
    base_url: String,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for LiveServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn spawn_server(state: AppState) -> LiveServer {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind test server");
    let address = listener.local_addr().expect("test server address");
    let router = ApiServer::from_state(state).into_router();
    let task = tokio::spawn(async move {
        axum::serve(listener, router)
            .await
            .expect("serve interaction SSE fixture");
    });
    LiveServer {
        base_url: format!("http://{address}"),
        task,
    }
}

fn delayed_interaction_state(config: Config) -> (AppState, AgentId) {
    let pool = SqlitePool::open_in_memory().expect("open SQLite fixture");
    migrations::migrate(&pool.writer()).expect("migrate SQLite fixture");
    let agent_id = AgentId::new();
    let spec = AgentSpec::new(agent_id, "sse-delayed-agent", "fake/model");
    let now = chrono::Utc::now().to_rfc3339();
    pool.writer()
        .execute(
            "INSERT INTO agents (id, name, state, spec_json, created_at, updated_at)
             VALUES (?1, ?2, 'active', ?3, ?4, ?4)",
            rusqlite::params![
                agent_id.to_string(),
                &spec.name,
                serde_json::to_string(&spec).expect("encode agent"),
                now
            ],
        )
        .expect("seed durable agent");

    let event_bus = EventBus::new(32);
    let shared_pool = Arc::new(pool.clone());
    let recorder = EventRecorder::new(shared_pool.clone(), event_bus.clone());
    let app = Arc::new(
        AppService::builder()
            .with_config(config.clone())
            .with_run_store(shared_pool.clone())
            .with_effect_store(shared_pool.clone())
            .with_conversation_store(shared_pool.clone())
            .with_payment_store(shared_pool.clone())
            .with_event_bus(event_bus.clone())
            .with_event_recorder(recorder)
            .build()
            .expect("build queued app service"),
    );
    app.create_agent(spec).expect("register live agent");
    let service: Arc<dyn InteractionService> = Arc::new(DurableInteractionService::new(app, pool));
    let state = AppState::new(
        config,
        Arc::new(InMemoryAgentStore::new()),
        Arc::new(InMemoryRunManager::new()),
        shared_pool.clone(),
        event_bus,
    )
    .with_conversation_store(shared_pool)
    .with_interaction_service(service);
    (state, agent_id)
}

#[derive(Debug)]
struct ParsedSseEvent {
    id: u64,
    name: String,
    envelope: InteractionEventEnvelope,
}

struct SseReader {
    response: reqwest::Response,
    buffered: Vec<u8>,
}

impl SseReader {
    async fn connect(
        client: &reqwest::Client,
        url: &str,
        last_event_id: Option<u64>,
        bearer: Option<&str>,
    ) -> Self {
        let mut request = client.get(url);
        if let Some(sequence) = last_event_id {
            request = request.header("Last-Event-ID", sequence.to_string());
        }
        if let Some(token) = bearer {
            request = request.bearer_auth(token);
        }
        let response = request.send().await.expect("connect interaction SSE");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .expect("SSE content type")
                .to_str()
                .expect("ASCII content type"),
            "text/event-stream"
        );
        Self {
            response,
            buffered: Vec::new(),
        }
    }

    fn pop_frame(&mut self) -> Option<String> {
        let end = self
            .buffered
            .windows(2)
            .position(|window| window == b"\n\n")?;
        let frame = self.buffered.drain(..end + 2).collect::<Vec<_>>();
        Some(String::from_utf8(frame[..end].to_vec()).expect("UTF-8 SSE frame"))
    }

    async fn next(&mut self) -> ParsedSseEvent {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                while let Some(frame) = self.pop_frame() {
                    if frame.starts_with(':') {
                        continue;
                    }
                    let mut id = None;
                    let mut name = None;
                    let mut data = Vec::new();
                    for line in frame.lines() {
                        let (field, value) = line.split_once(':').unwrap_or((line, ""));
                        let value = value.strip_prefix(' ').unwrap_or(value);
                        match field {
                            "id" => id = Some(value.parse::<u64>().expect("numeric SSE id")),
                            "event" => name = Some(value.to_owned()),
                            "data" => data.push(value),
                            _ => {}
                        }
                    }
                    let envelope =
                        serde_json::from_str::<InteractionEventEnvelope>(&data.join("\n"))
                            .expect("typed interaction envelope");
                    let id = id.expect("SSE id");
                    assert_eq!(id, envelope.sequence);
                    return ParsedSseEvent {
                        id,
                        name: name.expect("SSE event name"),
                        envelope,
                    };
                }
                let chunk = self
                    .response
                    .chunk()
                    .await
                    .expect("read SSE body")
                    .expect("SSE stream remains open");
                self.buffered.extend_from_slice(&chunk);
            }
        })
        .await
        .expect("SSE event delivery")
    }
}

async fn create_interaction(
    client: &reqwest::Client,
    server: &LiveServer,
    agent_id: AgentId,
) -> ConversationId {
    let response = client
        .post(format!("{}/api/v1alpha1/interactions", server.base_url))
        .json(&serde_json::json!({
            "target": {"kind": "agent", "id": agent_id},
            "working_directory": "/tmp"
        }))
        .send()
        .await
        .expect("create interaction");
    assert_eq!(response.status(), StatusCode::CREATED);
    response
        .json::<serde_json::Value>()
        .await
        .expect("create response")["interaction"]["conversation_id"]
        .as_str()
        .expect("conversation id")
        .parse()
        .expect("typed conversation id")
}

async fn prompt_turn(
    client: &reqwest::Client,
    server: &LiveServer,
    conversation_id: ConversationId,
    prompt: &str,
) -> InteractionTurnId {
    let turn_id = InteractionTurnId::new();
    let response = client
        .post(format!(
            "{}/api/v1alpha1/interactions/{conversation_id}/prompt",
            server.base_url
        ))
        .json(&serde_json::json!({
            "turn_id": turn_id,
            "prompt": prompt,
            "working_directory": "/tmp"
        }))
        .send()
        .await
        .expect("prompt interaction");
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    assert_eq!(
        response
            .json::<serde_json::Value>()
            .await
            .expect("prompt response")["handle"]["turn_id"],
        turn_id.to_string()
    );
    turn_id
}

async fn cancel_turn(
    client: &reqwest::Client,
    server: &LiveServer,
    conversation_id: ConversationId,
    turn_id: InteractionTurnId,
) {
    let response = client
        .post(format!(
            "{}/api/v1alpha1/interactions/{conversation_id}/turns/{turn_id}/cancel",
            server.base_url
        ))
        .send()
        .await
        .expect("cancel interaction");
    assert_eq!(response.status(), StatusCode::OK);
}

async fn through_terminal(reader: &mut SseReader) -> Vec<ParsedSseEvent> {
    let mut events = Vec::new();
    for _ in 0..32 {
        let event = reader.next().await;
        let terminal = event.envelope.event.is_terminal();
        events.push(event);
        if terminal {
            return events;
        }
    }
    panic!("interaction stream did not produce a terminal event");
}

#[tokio::test]
async fn sse_replays_follows_and_reconnects_exactly_once_with_turn_filtering() {
    let (state, agent_id) = delayed_interaction_state(Config::default());
    let server = spawn_server(state).await;
    let client = reqwest::Client::new();
    let conversation_id = create_interaction(&client, &server, agent_id).await;
    let first_turn = prompt_turn(&client, &server, conversation_id, "wait for cancellation").await;

    let stream_url = format!(
        "{}/api/v1alpha1/interactions/{conversation_id}/events/stream?after_sequence=0",
        server.base_url
    );
    let mut initial = SseReader::connect(&client, &stream_url, None, None).await;
    let first = initial.next().await;
    assert_eq!(first.name, "interaction_event");
    assert_eq!(first.envelope.turn_id, first_turn);
    assert_eq!(first.id, 1);
    drop(initial);

    // Last-Event-ID deliberately overrides the stale query checkpoint of 0.
    let mut resumed = SseReader::connect(&client, &stream_url, Some(first.id), None).await;
    cancel_turn(&client, &server, conversation_id, first_turn).await;
    let resumed_events = through_terminal(&mut resumed).await;
    assert_eq!(resumed_events[0].id, first.id + 1);
    assert!(matches!(
        resumed_events
            .last()
            .expect("terminal event")
            .envelope
            .event,
        InteractionEvent::TurnCancelled { .. }
    ));
    let all_sequences = std::iter::once(first.id)
        .chain(resumed_events.iter().map(|event| event.id))
        .collect::<Vec<_>>();
    assert_eq!(
        all_sequences,
        (1..=*all_sequences.last().expect("last sequence")).collect::<Vec<_>>()
    );
    let first_terminal_sequence = *all_sequences.last().expect("terminal sequence");

    let second_turn = prompt_turn(&client, &server, conversation_id, "filtered turn").await;
    let later_turn_event = resumed.next().await;
    assert!(later_turn_event.id > first_terminal_sequence);
    assert_eq!(later_turn_event.envelope.turn_id, second_turn);
    drop(resumed);

    let filtered_url = format!(
        "{}/api/v1alpha1/interactions/{conversation_id}/events/stream?after_sequence=0&turn_id={second_turn}",
        server.base_url
    );
    let mut filtered = SseReader::connect(&client, &filtered_url, None, None).await;
    let first_filtered = filtered.next().await;
    assert!(first_filtered.id > first_terminal_sequence);
    assert_eq!(first_filtered.envelope.turn_id, second_turn);
    cancel_turn(&client, &server, conversation_id, second_turn).await;
    let mut filtered_events = vec![first_filtered];
    filtered_events.extend(through_terminal(&mut filtered).await);
    assert!(filtered_events
        .iter()
        .all(|event| event.name == "interaction_event"));
    assert!(filtered_events
        .iter()
        .all(|event| event.envelope.turn_id == second_turn));
    drop(filtered);
}

#[tokio::test]
async fn sse_inherits_auth_read_only_and_uncomposed_boundaries() {
    let token = "interaction-sse-reader";
    let mut config = Config::default();
    config.auth.enabled = true;
    config.auth.api_keys = vec![format!("{:x}", Sha256::digest(token.as_bytes()))];
    config.api.read_only = true;
    let (state, _) = delayed_interaction_state(config);
    let server = spawn_server(state).await;
    let client = reqwest::Client::new();
    let missing = ConversationId::new();
    let url = format!(
        "{}/api/v1alpha1/interactions/{missing}/events/stream",
        server.base_url
    );
    assert_eq!(
        client
            .get(&url)
            .send()
            .await
            .expect("unauthenticated request")
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        client
            .get(&url)
            .bearer_auth(token)
            .send()
            .await
            .expect("authenticated read-only request")
            .status(),
        StatusCode::NOT_FOUND
    );

    let pool = Arc::new(SqlitePool::open_in_memory().expect("open uncomposed store"));
    let state = AppState::new(
        Config::default(),
        Arc::new(InMemoryAgentStore::new()),
        Arc::new(InMemoryRunManager::new()),
        pool,
        EventBus::new(8),
    );
    TestServer::new(ApiServer::from_state(state).into_router())
        .get(&format!(
            "/api/v1alpha1/interactions/{missing}/events/stream"
        ))
        .await
        .assert_status(StatusCode::NOT_IMPLEMENTED);

    TestServer::new(
        ApiServer::from_state(AppState::new(
            Config::default(),
            Arc::new(InMemoryAgentStore::new()),
            Arc::new(InMemoryRunManager::new()),
            Arc::new(SqlitePool::open_in_memory().expect("open malformed-header store")),
            EventBus::new(8),
        ))
        .into_router(),
    )
    .get(&format!(
        "/api/v1alpha1/interactions/{missing}/events/stream?after_sequence=42"
    ))
    .add_header("Last-Event-ID", "not-a-sequence")
    .await
    .assert_status(StatusCode::UNPROCESSABLE_ENTITY);
}

struct ScriptedStream {
    events: VecDeque<Result<InteractionEventEnvelope, StreamError>>,
    checkpoint: Option<u64>,
}

#[async_trait]
impl InteractionEventStream for ScriptedStream {
    async fn recv(&mut self) -> Result<InteractionEventEnvelope, StreamError> {
        let result = self.events.pop_front().unwrap_or(Err(StreamError::Closed));
        if let Ok(event) = &result {
            self.checkpoint = Some(event.sequence);
        }
        result
    }

    fn checkpoint(&self) -> Option<u64> {
        self.checkpoint
    }
}

struct LagRecoveryService {
    conversation_id: ConversationId,
    turn_id: InteractionTurnId,
    subscriptions: Mutex<Vec<SubscriptionRequest>>,
    calls: AtomicUsize,
    backend_failure: bool,
}

impl LagRecoveryService {
    fn envelope(&self, sequence: u64, terminal: bool) -> InteractionEventEnvelope {
        InteractionEventEnvelope {
            event_id: polkagent_interaction::InteractionEventId::new(),
            conversation_id: self.conversation_id,
            turn_id: self.turn_id,
            sequence,
            timestamp: chrono::Utc::now(),
            event: if terminal {
                InteractionEvent::TurnTimedOut
            } else {
                InteractionEvent::AgentMessageDelta {
                    run_id: RunId::new(),
                    text: format!("event-{sequence}"),
                }
            },
        }
    }

    fn unsupported() -> InteractionError {
        InteractionError::new(InteractionErrorCode::Unsupported, "not used by SSE fixture")
    }
}

#[async_trait]
impl InteractionService for LagRecoveryService {
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

    async fn list_turns(
        &self,
        _conversation_id: ConversationId,
    ) -> Result<Vec<TurnSummary>, InteractionError> {
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

    async fn approve(
        &self,
        _conversation_id: ConversationId,
        _approval_id: polkagent_core::ApprovalId,
    ) -> Result<polkagent_interaction::ApprovalView, InteractionError> {
        Err(Self::unsupported())
    }

    async fn deny(
        &self,
        _conversation_id: ConversationId,
        _approval_id: polkagent_core::ApprovalId,
        _reason: Option<String>,
    ) -> Result<polkagent_interaction::ApprovalView, InteractionError> {
        Err(Self::unsupported())
    }

    async fn subscribe(
        &self,
        request: SubscriptionRequest,
    ) -> Result<BoxInteractionEventStream, InteractionError> {
        self.subscriptions.lock().await.push(request);
        if self.backend_failure {
            return Ok(Box::new(ScriptedStream {
                events: VecDeque::from([
                    Err(StreamError::Backend(InteractionError::new(
                        InteractionErrorCode::Internal,
                        PRIVATE_BACKEND_MESSAGE,
                    ))),
                    Err(StreamError::Closed),
                ]),
                checkpoint: None,
            }));
        }
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        let events = if call == 0 {
            VecDeque::from([
                Ok(self.envelope(1, false)),
                Err(StreamError::Lagged {
                    last_seen_sequence: Some(1),
                    resume_after_sequence: 99,
                }),
            ])
        } else {
            VecDeque::from([
                Ok(self.envelope(1, false)),
                Ok(self.envelope(2, false)),
                Ok(self.envelope(3, true)),
                Err(StreamError::Closed),
            ])
        };
        Ok(Box::new(ScriptedStream {
            events,
            checkpoint: None,
        }))
    }
}

fn parse_complete_sse(body: &str) -> Vec<ParsedSseEvent> {
    body.split("\n\n")
        .filter(|frame| !frame.is_empty() && !frame.starts_with(':'))
        .map(|frame| {
            let mut id = None;
            let mut name = None;
            let mut data = None;
            for line in frame.lines() {
                let (field, value) = line.split_once(':').unwrap_or((line, ""));
                let value = value.strip_prefix(' ').unwrap_or(value);
                match field {
                    "id" => id = Some(value.parse::<u64>().expect("numeric SSE id")),
                    "event" => name = Some(value.to_owned()),
                    "data" => data = Some(value.to_owned()),
                    _ => {}
                }
            }
            ParsedSseEvent {
                id: id.expect("SSE id"),
                name: name.expect("SSE event name"),
                envelope: serde_json::from_str(&data.expect("SSE data"))
                    .expect("typed SSE envelope"),
            }
        })
        .collect()
}

#[tokio::test]
async fn deterministic_lag_replays_after_last_emitted_sequence_without_duplicates() {
    let conversation_id = ConversationId::new();
    let turn_id = InteractionTurnId::new();
    let service = Arc::new(LagRecoveryService {
        conversation_id,
        turn_id,
        subscriptions: Mutex::new(Vec::new()),
        calls: AtomicUsize::new(0),
        backend_failure: false,
    });
    let pool = Arc::new(SqlitePool::open_in_memory().expect("open lag fixture"));
    let state = AppState::new(
        Config::default(),
        Arc::new(InMemoryAgentStore::new()),
        Arc::new(InMemoryRunManager::new()),
        pool,
        EventBus::new(8),
    )
    .with_interaction_service(service.clone());
    let response = TestServer::new(ApiServer::from_state(state).into_router())
        .get(&format!(
            "/api/v1alpha1/interactions/{conversation_id}/events/stream?after_sequence=0"
        ))
        .await;
    response.assert_status_ok();
    response.assert_header("content-type", "text/event-stream");
    let events = parse_complete_sse(&response.text());
    assert_eq!(
        events.iter().map(|event| event.id).collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
    assert!(events.iter().all(|event| event.name == "interaction_event"));
    assert!(events
        .iter()
        .all(|event| event.id == event.envelope.sequence));
    let subscriptions = service.subscriptions.lock().await;
    assert_eq!(subscriptions.len(), 2);
    assert_eq!(subscriptions[0].after_sequence, Some(0));
    assert_eq!(subscriptions[1].after_sequence, Some(1));
}

#[derive(Clone, Default)]
struct CapturedLogs(Arc<StdMutex<Vec<u8>>>);

struct CapturedWriter(CapturedLogs);

impl Write for CapturedWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0
             .0
            .lock()
            .expect("capture log lock")
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'writer> MakeWriter<'writer> for CapturedLogs {
    type Writer = CapturedWriter;

    fn make_writer(&'writer self) -> Self::Writer {
        CapturedWriter(self.clone())
    }
}

impl CapturedLogs {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().expect("capture log lock").clone())
            .expect("UTF-8 captured logs")
    }
}

#[tokio::test(flavor = "current_thread")]
async fn backend_stream_failure_logs_only_typed_code_and_ends_safely() {
    let conversation_id = ConversationId::new();
    let service = Arc::new(LagRecoveryService {
        conversation_id,
        turn_id: InteractionTurnId::new(),
        subscriptions: Mutex::new(Vec::new()),
        calls: AtomicUsize::new(0),
        backend_failure: true,
    });
    let state = AppState::new(
        Config::default(),
        Arc::new(InMemoryAgentStore::new()),
        Arc::new(InMemoryRunManager::new()),
        Arc::new(SqlitePool::open_in_memory().expect("open backend-failure fixture")),
        EventBus::new(8),
    )
    .with_interaction_service(service);
    let logs = CapturedLogs::default();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(logs.clone())
        .finish();
    let dispatch = tracing::Dispatch::new(subscriber);
    let _guard = tracing::dispatcher::set_default(&dispatch);

    let response = TestServer::new(ApiServer::from_state(state).into_router())
        .get(&format!(
            "/api/v1alpha1/interactions/{conversation_id}/events/stream"
        ))
        .await;
    response.assert_status_ok();
    response.assert_header("content-type", "text/event-stream");
    assert!(response.text().is_empty());

    let logs = logs.text();
    assert!(
        logs.contains("error_code=internal"),
        "captured logs: {logs}"
    );
    assert!(!logs.contains(PRIVATE_BACKEND_MESSAGE));
}
