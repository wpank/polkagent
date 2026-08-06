//! Black-box coverage for checkpoint-aware durable run-event `WebSocket`s.

#![allow(
    clippy::expect_used,
    clippy::too_many_lines,
    reason = "integration fixtures fail at explicit WebSocket and durability boundaries"
)]

use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};
use std::time::Duration;

use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use polkagent_api::{ApiServer, AppState, InMemoryAgentStore, InMemoryRunManager, RunManagerTrait};
use polkagent_config::Config;
use polkagent_core::{
    event::{EventCorrelation, EventKind, RunEvent},
    AgentId, EffectAttemptId, EffectId, EventId, RunId, StepId, TurnId,
};
use polkagent_event::{types::EventType, EventBus, EventRecorder};
use polkagent_store_sqlite::{migrations, SqlitePool};
use polkagent_store_trait::event::{EventFilter, EventStore, EventStoreError, StoredEvent};
use sha2::{Digest, Sha256};
use tokio::sync::{Notify, RwLock};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{
        client::IntoClientRequest,
        http::{header::AUTHORIZATION, HeaderValue, StatusCode},
        protocol::frame::coding::CloseCode,
        Error as WebSocketError, Message,
    },
    MaybeTlsStream, WebSocketStream,
};

const PRIVATE_BACKEND_SENTINEL: &str = "PRIVATE_EVENT_STREAM_BACKEND_SENTINEL";

type ClientSocket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

struct LiveServer {
    ws_base_url: String,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for LiveServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn spawn_server(
    config: Config,
    bus: EventBus,
    store: Option<Arc<dyn EventStore>>,
) -> LiveServer {
    spawn_server_with_run_manager(config, bus, store, Arc::new(InMemoryRunManager::new())).await
}

async fn spawn_server_with_run_manager(
    config: Config,
    bus: EventBus,
    store: Option<Arc<dyn EventStore>>,
    run_manager: Arc<dyn RunManagerTrait>,
) -> LiveServer {
    let pool = SqlitePool::open_in_memory().expect("open effect-store fixture");
    migrations::migrate(&pool.writer()).expect("migrate effect-store fixture");
    let mut state = AppState::new(
        config,
        Arc::new(InMemoryAgentStore::new()),
        run_manager,
        Arc::new(pool),
        bus,
    );
    if let Some(store) = store {
        state = state.with_event_store(store);
    }

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind event stream fixture");
    let address = listener.local_addr().expect("event stream address");
    let router = ApiServer::from_state(state).into_router();
    let task = tokio::spawn(async move {
        axum::serve(listener, router)
            .await
            .expect("serve event stream fixture");
    });
    LiveServer {
        ws_base_url: format!("ws://{address}"),
        task,
    }
}

async fn connect(server: &LiveServer, query: &str, bearer: Option<&str>) -> ClientSocket {
    let mut request = format!("{}/api/v1alpha1/events/stream{query}", server.ws_base_url)
        .into_client_request()
        .expect("WebSocket request");
    if let Some(token) = bearer {
        request.headers_mut().insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {token}")).expect("authorization header"),
        );
    }
    connect_async(request)
        .await
        .expect("connect event WebSocket")
        .0
}

async fn connect_command(server: &LiveServer) -> ClientSocket {
    connect_async(format!(
        "{}/ws/v1alpha1?token=test-token",
        server.ws_base_url
    ))
    .await
    .expect("connect command WebSocket")
    .0
}

async fn connect_command_at_cursor(server: &LiveServer, cursor: &str) -> ClientSocket {
    connect_async(format!(
        "{}/ws/v1alpha1?token=test-token&cursor={cursor}",
        server.ws_base_url
    ))
    .await
    .expect("connect checkpointed command WebSocket")
    .0
}

async fn send_command(socket: &mut ClientSocket, message: serde_json::Value) {
    socket
        .send(Message::Text(message.to_string().into()))
        .await
        .expect("send command frame");
}

async fn subscribe(socket: &mut ClientSocket, channel: &str, request_id: &str) {
    send_command(
        socket,
        serde_json::json!({
            "msg_type": "subscribe",
            "id": request_id,
            "channel": channel,
        }),
    )
    .await;
    let ack = next_json(socket).await;
    assert_eq!(ack["msg_type"], "ack");
    assert_eq!(ack["id"], request_id);
    assert_eq!(ack["channel"], channel);
}

async fn unsubscribe(socket: &mut ClientSocket, channel: &str, request_id: &str) {
    send_command(
        socket,
        serde_json::json!({
            "msg_type": "unsubscribe",
            "id": request_id,
            "channel": channel,
        }),
    )
    .await;
    let ack = next_json(socket).await;
    assert_eq!(ack["msg_type"], "ack");
    assert_eq!(ack["id"], request_id);
    assert_eq!(ack["channel"], channel);
}

async fn next_json(socket: &mut ClientSocket) -> serde_json::Value {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match socket.next().await.expect("WebSocket remains open") {
                Ok(Message::Text(text)) => {
                    return serde_json::from_str(&text).expect("event frame JSON");
                }
                Ok(Message::Ping(payload)) => {
                    socket
                        .send(Message::Pong(payload))
                        .await
                        .expect("reply to ping");
                }
                Ok(Message::Close(frame)) => panic!("unexpected close frame: {frame:?}"),
                Ok(_) => {}
                Err(error) => panic!("WebSocket read failed: {error}"),
            }
        }
    })
    .await
    .expect("event frame delivery")
}

async fn expect_no_frame(socket: &mut ClientSocket) {
    let result = tokio::time::timeout(Duration::from_millis(150), socket.next()).await;
    assert!(
        result.is_err(),
        "unexpected duplicate WebSocket frame: {result:?}"
    );
}

fn http_error_status(error: WebSocketError) -> StatusCode {
    match error {
        WebSocketError::Http(response) => response.status(),
        other => panic!("expected HTTP handshake failure, got {other}"),
    }
}

#[derive(Default)]
struct TestEventStore {
    events: RwLock<Vec<StoredEvent>>,
    reads: RwLock<Vec<(u64, usize)>>,
    read_calls: AtomicUsize,
    fail_on_read: AtomicUsize,
    lookup_calls: AtomicUsize,
    block_lookup_on_call: AtomicUsize,
    block_first_read: AtomicBool,
    first_snapshot_taken: Notify,
    release_first_read: Notify,
    blocked_lookup_started: Notify,
    release_blocked_lookup: Notify,
}

impl TestEventStore {
    async fn insert(&self, event: StoredEvent) {
        self.events.write().await.push(event);
    }

    async fn reads(&self) -> Vec<(u64, usize)> {
        self.reads.read().await.clone()
    }

    fn fail_on_read(&self, call: usize) {
        self.fail_on_read.store(call, Ordering::SeqCst);
    }

    fn block_first_read(&self) {
        self.block_first_read.store(true, Ordering::SeqCst);
    }

    fn block_lookup_on_call(&self, call: usize) {
        self.block_lookup_on_call.store(call, Ordering::SeqCst);
    }
}

#[async_trait]
impl EventStore for TestEventStore {
    async fn append_durable(&self, event: StoredEvent) -> Result<StoredEvent, EventStoreError> {
        self.insert(event.clone()).await;
        Ok(event)
    }

    async fn append_diagnostic(
        &self,
        _event: StoredEvent,
        _expires_at: String,
    ) -> Result<(), EventStoreError> {
        Ok(())
    }

    async fn read_from_cursor(
        &self,
        cursor: u64,
        limit: usize,
    ) -> Result<Vec<StoredEvent>, EventStoreError> {
        self.reads.write().await.push((cursor, limit));
        let call = self.read_calls.fetch_add(1, Ordering::SeqCst) + 1;
        if self.fail_on_read.load(Ordering::SeqCst) == call {
            return Err(EventStoreError::Backend(Box::new(std::io::Error::other(
                PRIVATE_BACKEND_SENTINEL,
            ))));
        }

        let page = self
            .events
            .read()
            .await
            .iter()
            .filter(|event| event.global_sequence > cursor)
            .take(limit)
            .cloned()
            .collect();
        if call == 1 && self.block_first_read.load(Ordering::SeqCst) {
            self.first_snapshot_taken.notify_one();
            self.release_first_read.notified().await;
        }
        Ok(page)
    }

    async fn get_event_by_id(&self, id: &str) -> Result<StoredEvent, EventStoreError> {
        let call = self.lookup_calls.fetch_add(1, Ordering::SeqCst) + 1;
        let event = self
            .events
            .read()
            .await
            .iter()
            .find(|event| event.id == id)
            .cloned()
            .ok_or_else(|| EventStoreError::NotFound(format!("event {id}")))?;
        if self.block_lookup_on_call.load(Ordering::SeqCst) == call {
            self.blocked_lookup_started.notify_one();
            self.release_blocked_lookup.notified().await;
        }
        Ok(event)
    }

    async fn read_run_events(&self, run_id: RunId) -> Result<Vec<StoredEvent>, EventStoreError> {
        Ok(self
            .events
            .read()
            .await
            .iter()
            .filter(|event| event.run_id == run_id.to_string())
            .cloned()
            .collect())
    }

    async fn query(&self, filter: EventFilter) -> Result<Vec<StoredEvent>, EventStoreError> {
        let mut events = self.events.read().await.clone();
        events.retain(|event| {
            filter
                .run_id
                .is_none_or(|run_id| event.run_id == run_id.to_string())
                && (filter.event_types.is_empty() || filter.event_types.contains(&event.event_type))
                && filter
                    .since_global_sequence
                    .is_none_or(|sequence| event.global_sequence >= sequence)
        });
        if let Some(limit) = filter.limit {
            events.truncate(limit);
        }
        Ok(events)
    }

    async fn max_sequence(&self, run_id: RunId) -> Result<u64, EventStoreError> {
        Ok(self
            .events
            .read()
            .await
            .iter()
            .filter(|event| event.run_id == run_id.to_string())
            .map(|event| event.sequence)
            .max()
            .unwrap_or(0))
    }

    async fn has_terminal_event(&self, _run_id: RunId) -> Result<bool, EventStoreError> {
        Ok(false)
    }
}

fn stored_event(global_sequence: u64, run_id: RunId, kind: EventKind) -> (StoredEvent, RunEvent) {
    let id = EventId::new();
    let event_type = EventType::from_kind(&kind)
        .expect("fixture uses catalogued event kind")
        .as_str()
        .to_owned();
    let event = RunEvent::new_durable(
        id,
        run_id,
        global_sequence,
        kind.clone(),
        EventCorrelation {
            run_id,
            ..Default::default()
        },
    );
    let stored = StoredEvent {
        id: id.to_string(),
        event_type,
        sequence: global_sequence,
        global_sequence,
        run_id: run_id.to_string(),
        turn_id: None,
        step_id: None,
        effect_intent_id: None,
        effect_attempt_id: None,
        conversation_id: None,
        correlation_id: id.to_string(),
        causation_id: None,
        scope_id: "test".to_owned(),
        timestamp: event.timestamp.to_rfc3339(),
        durability: "durable".to_owned(),
        payload: serde_json::to_value(kind).expect("serialize fixture event kind"),
        trace_id: None,
        span_id: None,
        schema_version: 1,
    };
    (stored, event)
}

fn seed_sqlite_run(pool: &SqlitePool, run_id: RunId, agent_id: AgentId) {
    let now = chrono::Utc::now().to_rfc3339();
    let writer = pool.writer();
    writer
        .execute(
            "INSERT INTO agents (id, name, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?3)",
            rusqlite::params![agent_id.to_string(), "sqlite-stream-agent", now],
        )
        .expect("seed SQLite agent");
    writer
        .execute(
            "INSERT INTO runs (id, agent_id, state, created_at, updated_at)
             VALUES (?1, ?2, 'created', ?3, ?3)",
            rusqlite::params![run_id.to_string(), agent_id.to_string(), now],
        )
        .expect("seed SQLite run");
}

#[tokio::test]
async fn canonical_recorder_replays_real_sqlite_payload_and_rowid_checkpoint() {
    let pool = SqlitePool::open_in_memory().expect("open SQLite event fixture");
    migrations::migrate(&pool.writer()).expect("migrate SQLite event fixture");
    let run_id = RunId::new();
    seed_sqlite_run(&pool, run_id, AgentId::new());
    let bus = EventBus::new(8);
    let event_store: Arc<dyn EventStore> = Arc::new(pool.clone());
    let recorder = EventRecorder::new(event_store.clone(), bus.clone());
    let cause_id = EventId::new();
    recorder
        .record(RunEvent::new_durable(
            cause_id,
            run_id,
            0,
            EventKind::RunCreated,
            EventCorrelation {
                run_id,
                ..Default::default()
            },
        ))
        .await
        .expect("record canonical RunCreated");

    let server = spawn_server(Config::default(), bus, Some(event_store)).await;
    let mut first_connection = connect(&server, "?after_sequence=0", None).await;
    let first = next_json(&mut first_connection).await;
    assert_eq!(first["run_id"], run_id.to_string());
    assert_eq!(first["sequence"], 1);
    assert_eq!(first["kind"], "run_created");
    assert!(first["correlation"]["turn_id"].is_null());
    assert!(first["correlation"]["step_id"].is_null());
    assert!(first["correlation"]["effect_intent_id"].is_null());
    assert!(first["correlation"]["effect_attempt_id"].is_null());
    assert!(first["causation_id"].is_null());
    let first_global = first["global_sequence"]
        .as_u64()
        .expect("SQLite rowid checkpoint");
    assert!(first_global > 0);
    first_connection
        .close(None)
        .await
        .expect("close first SQLite stream");

    let turn_id = TurnId::new();
    let step_id = StepId::new();
    let effect_intent_id = EffectId::new();
    let effect_attempt_id = EffectAttemptId::new();
    let mut correlated = RunEvent::new_durable(
        EventId::new(),
        run_id,
        0,
        EventKind::RunStarted,
        EventCorrelation {
            run_id,
            turn_id: Some(turn_id),
            step_id: Some(step_id),
            effect_intent_id: Some(effect_intent_id),
            effect_attempt_id: Some(effect_attempt_id),
        },
    );
    correlated.causation_id = Some(cause_id);
    recorder
        .record(correlated)
        .await
        .expect("record canonical RunStarted");
    let mut resumed = connect(&server, &format!("?after_sequence={first_global}"), None).await;
    let second = next_json(&mut resumed).await;
    assert_eq!(second["sequence"], 2);
    assert_eq!(second["kind"], "run_started");
    assert_eq!(second["correlation"]["run_id"], run_id.to_string());
    assert_eq!(second["correlation"]["turn_id"], turn_id.to_string());
    assert_eq!(second["correlation"]["step_id"], step_id.to_string());
    assert_eq!(
        second["correlation"]["effect_intent_id"],
        effect_intent_id.to_string()
    );
    assert_eq!(
        second["correlation"]["effect_attempt_id"],
        effect_attempt_id.to_string()
    );
    assert_eq!(second["causation_id"], cause_id.to_string());
    assert!(second["global_sequence"].as_u64() > Some(first_global));
}

#[tokio::test]
async fn real_sqlite_event_type_mismatch_fails_closed_at_api_replay_boundary() {
    let pool = SqlitePool::open_in_memory().expect("open SQLite event fixture");
    migrations::migrate(&pool.writer()).expect("migrate SQLite event fixture");
    let run_id = RunId::new();
    seed_sqlite_run(&pool, run_id, AgentId::new());
    let event_id = EventId::new();
    pool.writer()
        .execute(
            "INSERT INTO run_events
                (id, run_id, sequence, kind, data_json, timestamp,
                 correlation_id, schema_version)
             VALUES (?1, ?2, 1, 'run_started', '\"run_created\"',
                     '2024-01-01T00:00:00Z', ?1, 1)",
            rusqlite::params![event_id.to_string(), run_id.to_string()],
        )
        .expect("seed mismatched persisted event type");

    let event_store: Arc<dyn EventStore> = Arc::new(pool);
    let server = spawn_server(Config::default(), EventBus::new(4), Some(event_store)).await;
    let mut socket = connect(&server, "?after_sequence=0", None).await;
    let message = tokio::time::timeout(Duration::from_secs(5), socket.next())
        .await
        .expect("explicit close delivery")
        .expect("close frame")
        .expect("valid close frame");
    let Message::Close(Some(frame)) = message else {
        panic!("expected explicit close frame, got {message:?}");
    };
    assert_eq!(frame.code, CloseCode::Error);
    assert_eq!(frame.reason, "durable event recovery invalid");
}

#[tokio::test]
async fn replays_follows_reconnects_and_filters_across_bounded_pages() {
    let target_run = RunId::new();
    let unrelated_run = RunId::new();
    let store = Arc::new(TestEventStore::default());
    for sequence in 1..=258 {
        let (stored, _) = if sequence == 1 || sequence == 258 {
            stored_event(sequence, target_run, EventKind::RunCreated)
        } else {
            stored_event(sequence, unrelated_run, EventKind::RunQueued)
        };
        store.insert(stored).await;
    }
    let bus = EventBus::new(8);
    let server = spawn_server(Config::default(), bus.clone(), Some(store.clone())).await;
    let query = format!("?after_sequence=0&run_id={target_run}&kinds=run_created");
    let mut socket = connect(&server, &query, None).await;

    let first = next_json(&mut socket).await;
    let across_page_boundary = next_json(&mut socket).await;
    assert_eq!(first["global_sequence"], 1);
    assert_eq!(across_page_boundary["global_sequence"], 258);
    assert_eq!(first["run_id"], target_run.to_string());
    assert_eq!(across_page_boundary["run_id"], target_run.to_string());
    let reads = store.reads().await;
    assert_eq!(reads[0], (0, 256));
    assert_eq!(reads[1], (256, 256));
    assert!(reads.iter().all(|(_, limit)| *limit == 256));
    socket.close(None).await.expect("close initial socket");

    let (reconnected_stored, _) = stored_event(259, target_run, EventKind::RunCreated);
    store.insert(reconnected_stored).await;
    let reconnect_query = format!("?after_sequence=258&run_id={target_run}&kinds=run_created");
    let mut resumed = connect(&server, &reconnect_query, None).await;
    assert_eq!(next_json(&mut resumed).await["global_sequence"], 259);

    let (live_stored, live) = stored_event(260, target_run, EventKind::RunCreated);
    store.insert(live_stored).await;
    bus.publish(live);
    assert_eq!(next_json(&mut resumed).await["global_sequence"], 260);
    expect_no_frame(&mut resumed).await;
}

#[tokio::test]
async fn forced_broadcast_lag_recovers_after_last_durable_checkpoint_without_duplicates() {
    let run_id = RunId::new();
    let store = Arc::new(TestEventStore::default());
    for sequence in 1..=2 {
        let (stored, _) = stored_event(sequence, run_id, EventKind::RunCreated);
        store.insert(stored).await;
    }
    store.block_first_read();
    let bus = EventBus::new(2);
    let server = spawn_server(Config::default(), bus.clone(), Some(store.clone())).await;
    let mut socket = connect(&server, "?after_sequence=0", None).await;
    store.first_snapshot_taken.notified().await;

    for sequence in 3..=6 {
        let (stored, live) = stored_event(sequence, run_id, EventKind::RunCreated);
        store.insert(stored).await;
        bus.publish(live);
    }
    store.release_first_read.notify_one();

    let mut delivered = Vec::new();
    for _ in 1..=6 {
        delivered.push(
            next_json(&mut socket).await["global_sequence"]
                .as_u64()
                .expect("durable checkpoint"),
        );
    }
    assert_eq!(delivered, (1..=6).collect::<Vec<_>>());
    expect_no_frame(&mut socket).await;
    let reads = store.reads().await;
    assert_eq!(reads[0], (0, 256));
    assert_eq!(reads[1], (2, 256));
    assert!(reads.iter().all(|(_, limit)| *limit == 256));
}

#[tokio::test]
async fn backend_failure_closes_explicitly_without_exposing_backend_detail() {
    let run_id = RunId::new();
    let store = Arc::new(TestEventStore::default());
    let (first_stored, _) = stored_event(1, run_id, EventKind::RunCreated);
    store.insert(first_stored).await;
    store.block_first_read();
    store.fail_on_read(2);
    let bus = EventBus::new(1);
    let server = spawn_server(Config::default(), bus.clone(), Some(store.clone())).await;
    let mut socket = connect(&server, "?after_sequence=0", None).await;
    store.first_snapshot_taken.notified().await;
    for sequence in 2..=4 {
        let (stored, live) = stored_event(sequence, run_id, EventKind::RunCreated);
        store.insert(stored).await;
        bus.publish(live);
    }
    store.release_first_read.notify_one();
    assert_eq!(next_json(&mut socket).await["global_sequence"], 1);

    let message = tokio::time::timeout(Duration::from_secs(5), socket.next())
        .await
        .expect("explicit close delivery")
        .expect("close frame")
        .expect("valid close frame");
    let Message::Close(Some(frame)) = message else {
        panic!("expected explicit close frame, got {message:?}");
    };
    assert_eq!(frame.code, CloseCode::Error);
    assert_eq!(frame.reason, "durable event recovery unavailable");
    assert!(!frame.reason.contains(PRIVATE_BACKEND_SENTINEL));
    assert_eq!(store.reads().await[..2], [(0, 256), (1, 256)]);
}

#[tokio::test]
async fn validates_checkpoint_requires_durable_store_and_preserves_auth() {
    let store = Arc::new(TestEventStore::default());
    let run_id = RunId::new();
    let (stored, _) = stored_event(1, run_id, EventKind::RunCreated);
    store.insert(stored).await;
    let token = "event-stream-secret";
    let mut config = Config::default();
    config.auth.enabled = true;
    config.auth.api_keys = vec![format!("{:x}", Sha256::digest(token.as_bytes()))];
    let server = spawn_server(config, EventBus::new(4), Some(store)).await;

    let unauthorized = connect_async(format!(
        "{}/api/v1alpha1/events/stream?after_sequence=0",
        server.ws_base_url
    ))
    .await
    .expect_err("missing authentication must reject upgrade");
    assert_eq!(http_error_status(unauthorized), StatusCode::UNAUTHORIZED);

    let invalid = {
        let mut request = format!(
            "{}/api/v1alpha1/events/stream?after_sequence=-1",
            server.ws_base_url
        )
        .into_client_request()
        .expect("invalid-checkpoint request");
        request.headers_mut().insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {token}")).expect("authorization header"),
        );
        connect_async(request)
            .await
            .expect_err("negative checkpoint must reject upgrade")
    };
    assert_eq!(http_error_status(invalid), StatusCode::UNPROCESSABLE_ENTITY);

    let mut authenticated = connect(&server, "?after_sequence=0", Some(token)).await;
    assert_eq!(next_json(&mut authenticated).await["global_sequence"], 1);

    let missing_store_server = spawn_server(Config::default(), EventBus::new(4), None).await;
    let unavailable = connect_async(format!(
        "{}/api/v1alpha1/events/stream?after_sequence=0",
        missing_store_server.ws_base_url
    ))
    .await
    .expect_err("missing durable store must reject upgrade");
    assert_eq!(http_error_status(unavailable), StatusCode::NOT_IMPLEMENTED);
}

#[tokio::test]
async fn command_socket_forced_lag_recovers_in_session_without_duplicates() {
    let run_id = RunId::new();
    let store = Arc::new(TestEventStore::default());
    let bus = EventBus::new(2);
    let server = spawn_server(Config::default(), bus.clone(), Some(store.clone())).await;
    let mut socket = connect_command(&server).await;
    subscribe(&mut socket, &format!("runs:{run_id}"), "subscribe-run").await;

    let (first_stored, first_live) = stored_event(1, run_id, EventKind::RunCreated);
    store.insert(first_stored).await;
    bus.publish(first_live);
    let first = next_json(&mut socket).await;
    assert_eq!(first["msg_type"], "event");
    assert_eq!(first["payload"]["sequence"], 1);
    assert!(first["payload"].get("global_sequence").is_none());

    store.block_lookup_on_call(2);
    let (second_stored, second_live) = stored_event(2, run_id, EventKind::RunQueued);
    store.insert(second_stored).await;
    bus.publish(second_live);
    store.blocked_lookup_started.notified().await;
    for sequence in 3..=6 {
        let (stored, live) = stored_event(sequence, run_id, EventKind::RunQueued);
        store.insert(stored).await;
        bus.publish(live);
    }
    store.release_blocked_lookup.notify_one();

    let mut delivered = vec![1];
    for _ in 2..=6 {
        let message = next_json(&mut socket).await;
        assert_eq!(message["msg_type"], "event");
        delivered.push(
            message["payload"]["sequence"]
                .as_u64()
                .expect("run sequence"),
        );
    }
    assert_eq!(delivered, (1..=6).collect::<Vec<_>>());
    expect_no_frame(&mut socket).await;
    let reads = store.reads().await;
    assert_eq!(reads[0], (1, 256));
    assert!(
        reads.contains(&(6, 256)),
        "forced Lagged must replay after the last delivered checkpoint: {reads:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn command_socket_lag_before_checkpoint_closes_without_fabricating_replay() {
    let run_id = RunId::new();
    let store = Arc::new(TestEventStore::default());
    let bus = EventBus::new(1);
    let server = spawn_server(Config::default(), bus.clone(), Some(store.clone())).await;
    let mut socket = connect_command(&server).await;
    subscribe(&mut socket, &format!("runs:{run_id}"), "subscribe-run").await;

    let mut live_events = Vec::new();
    for sequence in 1..=16 {
        let (stored, live) = stored_event(sequence, run_id, EventKind::RunQueued);
        store.insert(stored).await;
        live_events.push(live);
    }
    // There is deliberately no await in this loop. On the current-thread
    // runtime the attached server receiver cannot observe the first event
    // before the bounded bus has overwritten it.
    for event in live_events {
        bus.publish(event);
    }

    let protocol_error = next_json(&mut socket).await;
    assert_eq!(protocol_error["msg_type"], "error");
    assert_eq!(
        protocol_error["payload"]["reason"],
        "durable event checkpoint unavailable"
    );
    let close = socket
        .next()
        .await
        .expect("explicit close message")
        .expect("valid close message");
    let Message::Close(Some(frame)) = close else {
        panic!("expected close frame, got {close:?}");
    };
    assert_eq!(frame.code, CloseCode::Error);
    assert_eq!(frame.reason, "durable event checkpoint unavailable");
    assert!(store.reads().await.is_empty());
}

#[tokio::test]
async fn command_socket_rejects_zero_global_sequence_from_point_lookup() {
    let run_id = RunId::new();
    let store = Arc::new(TestEventStore::default());
    let bus = EventBus::new(4);
    let server = spawn_server(Config::default(), bus.clone(), Some(store.clone())).await;
    let mut socket = connect_command(&server).await;
    subscribe(&mut socket, &format!("runs:{run_id}"), "subscribe-run").await;

    let (stored, live) = stored_event(0, run_id, EventKind::RunCreated);
    store.insert(stored).await;
    bus.publish(live);

    let protocol_error = next_json(&mut socket).await;
    assert_eq!(protocol_error["msg_type"], "error");
    assert_eq!(
        protocol_error["payload"]["reason"],
        "durable event recovery invalid"
    );
    let close = socket
        .next()
        .await
        .expect("explicit close message")
        .expect("valid close message");
    let Message::Close(Some(frame)) = close else {
        panic!("expected close frame, got {close:?}");
    };
    assert_eq!(frame.code, CloseCode::Error);
    assert_eq!(frame.reason, "durable event recovery invalid");
    assert!(store.reads().await.is_empty());
}

#[tokio::test]
async fn command_socket_filters_multiple_run_and_agent_subscriptions_and_unsubscribe() {
    let agent_a = AgentId::new();
    let agent_b = AgentId::new();
    let run_manager = Arc::new(InMemoryRunManager::new());
    let run_a = run_manager
        .create_run(agent_a, serde_json::json!({}))
        .await
        .expect("create run A")
        .id;
    let run_b = run_manager
        .create_run(agent_b, serde_json::json!({}))
        .await
        .expect("create run B")
        .id;
    let store = Arc::new(TestEventStore::default());
    let bus = EventBus::new(8);
    let server = spawn_server_with_run_manager(
        Config::default(),
        bus.clone(),
        Some(store.clone()),
        run_manager,
    )
    .await;
    let mut socket = connect_command(&server).await;
    let run_channel = format!("runs:{run_a}");
    let agent_channel = format!("agents:{agent_b}");
    subscribe(&mut socket, &run_channel, "subscribe-a").await;
    subscribe(&mut socket, &agent_channel, "subscribe-agent-b").await;

    let (a_stored, a_live) = stored_event(1, run_a, EventKind::RunCreated);
    store.insert(a_stored).await;
    bus.publish(a_live);
    let (b_stored, b_live) = stored_event(2, run_b, EventKind::RunCreated);
    store.insert(b_stored).await;
    bus.publish(b_live);
    assert_eq!(next_json(&mut socket).await["channel"], run_channel);
    assert_eq!(
        next_json(&mut socket).await["channel"],
        format!("runs:{run_b}")
    );

    unsubscribe(&mut socket, &run_channel, "unsubscribe-a").await;
    let (hidden_stored, hidden_live) = stored_event(3, run_a, EventKind::RunStarted);
    store.insert(hidden_stored).await;
    bus.publish(hidden_live);
    let (visible_stored, visible_live) = stored_event(4, run_b, EventKind::RunStarted);
    store.insert(visible_stored).await;
    bus.publish(visible_live);
    let visible = next_json(&mut socket).await;
    assert_eq!(visible["channel"], format!("runs:{run_b}"));
    assert_eq!(visible["payload"]["sequence"], 4);
    expect_no_frame(&mut socket).await;
}

#[tokio::test]
async fn command_socket_reconnect_is_truthfully_live_only_without_cursor_protocol() {
    let run_id = RunId::new();
    let store = Arc::new(TestEventStore::default());
    let bus = EventBus::new(8);
    let server = spawn_server(Config::default(), bus.clone(), Some(store.clone())).await;
    let channel = format!("runs:{run_id}");
    let mut first_socket = connect_command(&server).await;
    subscribe(&mut first_socket, &channel, "first-subscribe").await;
    let (first_stored, first_live) = stored_event(1, run_id, EventKind::RunCreated);
    store.insert(first_stored).await;
    bus.publish(first_live);
    assert_eq!(next_json(&mut first_socket).await["payload"]["sequence"], 1);
    first_socket
        .close(None)
        .await
        .expect("close command socket");

    let (disconnected_stored, disconnected_live) = stored_event(2, run_id, EventKind::RunQueued);
    store.insert(disconnected_stored).await;
    bus.publish(disconnected_live);

    let mut reconnected = connect_command(&server).await;
    subscribe(&mut reconnected, &channel, "reconnect-subscribe").await;
    expect_no_frame(&mut reconnected).await;
    let (future_stored, future_live) = stored_event(3, run_id, EventKind::RunStarted);
    store.insert(future_stored).await;
    bus.publish(future_live);
    let future = next_json(&mut reconnected).await;
    assert_eq!(future["payload"]["sequence"], 3);
    assert_ne!(future["payload"]["sequence"], 2);
}

#[tokio::test]
async fn command_socket_cursor_replays_filters_reconnects_and_dedupes_across_pages() {
    let target_run = RunId::new();
    let unrelated_run = RunId::new();
    let store = Arc::new(TestEventStore::default());
    for sequence in 1..=258 {
        let (stored, _) = if sequence == 1 || sequence == 258 {
            stored_event(sequence, target_run, EventKind::RunCreated)
        } else {
            stored_event(sequence, unrelated_run, EventKind::RunQueued)
        };
        store.insert(stored).await;
    }
    let bus = EventBus::new(8);
    let server = spawn_server(Config::default(), bus.clone(), Some(store.clone())).await;
    let channel = format!("runs:{target_run}");
    let mut initial = connect_command_at_cursor(&server, "v1:0").await;
    subscribe(&mut initial, &channel, "initial-subscribe").await;

    let first = next_json(&mut initial).await;
    let across_page = next_json(&mut initial).await;
    assert_eq!(first["payload"]["sequence"], 1);
    assert_eq!(first["cursor"], "v1:1");
    assert_eq!(across_page["payload"]["sequence"], 258);
    assert_eq!(across_page["cursor"], "v1:258");
    assert_eq!(first["channel"], channel);
    assert_eq!(across_page["channel"], channel);
    expect_no_frame(&mut initial).await;
    let initial_reads = store.reads().await;
    assert_eq!(initial_reads[..2], [(0, 256), (256, 256)]);
    assert!(initial_reads.iter().all(|(_, limit)| *limit <= 256));
    initial
        .close(None)
        .await
        .expect("close initial command socket");

    let (replay_stored, replay_live) = stored_event(259, target_run, EventKind::RunStarted);
    store.insert(replay_stored).await;
    let mut resumed = connect_command_at_cursor(&server, "v1:258").await;
    subscribe(&mut resumed, &channel, "resume-subscribe").await;
    let replayed = next_json(&mut resumed).await;
    assert_eq!(replayed["payload"]["sequence"], 259);
    assert_eq!(replayed["cursor"], "v1:259");

    // A live notification for an event already recovered from the store is a
    // wake-up only and must not duplicate the public checkpointed event.
    bus.publish(replay_live);
    let (future_stored, future_live) = stored_event(260, target_run, EventKind::RunStarted);
    store.insert(future_stored).await;
    bus.publish(future_live);
    let future = next_json(&mut resumed).await;
    assert_eq!(future["payload"]["sequence"], 260);
    assert_eq!(future["cursor"], "v1:260");
    expect_no_frame(&mut resumed).await;

    let reads = store.reads().await;
    assert!(
        reads.contains(&(257, 1)),
        "cursor validation is bounded: {reads:?}"
    );
    assert!(
        reads.contains(&(258, 256)),
        "resume replay is bounded: {reads:?}"
    );
}

#[tokio::test]
async fn command_socket_cursor_replay_and_lag_recovery_do_not_duplicate() {
    let run_id = RunId::new();
    let store = Arc::new(TestEventStore::default());
    let (first_stored, _) = stored_event(1, run_id, EventKind::RunCreated);
    store.insert(first_stored).await;
    let bus = EventBus::new(2);
    let server = spawn_server(Config::default(), bus.clone(), Some(store.clone())).await;
    let mut socket = connect_command_at_cursor(&server, "v1:0").await;
    subscribe(&mut socket, &format!("runs:{run_id}"), "cursor-lag").await;
    let first = next_json(&mut socket).await;
    assert_eq!(first["payload"]["sequence"], 1);
    assert_eq!(first["cursor"], "v1:1");

    store.block_lookup_on_call(1);
    let (second_stored, second_live) = stored_event(2, run_id, EventKind::RunQueued);
    store.insert(second_stored).await;
    bus.publish(second_live);
    store.blocked_lookup_started.notified().await;
    for sequence in 3..=6 {
        let (stored, live) = stored_event(sequence, run_id, EventKind::RunQueued);
        store.insert(stored).await;
        bus.publish(live);
    }
    store.release_blocked_lookup.notify_one();

    let mut delivered = vec![1];
    for expected in 2..=6 {
        let event = next_json(&mut socket).await;
        assert_eq!(event["cursor"], format!("v1:{expected}"));
        delivered.push(event["payload"]["sequence"].as_u64().expect("sequence"));
    }
    assert_eq!(delivered, (1..=6).collect::<Vec<_>>());
    expect_no_frame(&mut socket).await;
    let reads = store.reads().await;
    assert!(
        reads.contains(&(1, 256)),
        "lag recovery starts after cursor: {reads:?}"
    );
    assert!(
        reads.contains(&(6, 256)),
        "lag wake-up deduplicates: {reads:?}"
    );
}

#[tokio::test]
async fn command_socket_cursor_rejects_malformed_stale_and_future_tokens() {
    let run_id = RunId::new();
    let store = Arc::new(TestEventStore::default());
    let (stored, _) = stored_event(2, run_id, EventKind::RunCreated);
    store.insert(stored).await;
    let server = spawn_server(Config::default(), EventBus::new(4), Some(store.clone())).await;

    for cursor in ["garbage", "v2:2", "v1:-1", "v1:02"] {
        let error = connect_async(format!(
            "{}/ws/v1alpha1?token=test-token&cursor={cursor}",
            server.ws_base_url
        ))
        .await
        .expect_err("malformed cursor must reject upgrade");
        assert_eq!(http_error_status(error), StatusCode::UNPROCESSABLE_ENTITY);
    }

    let stale = connect_async(format!(
        "{}/ws/v1alpha1?token=test-token&cursor=v1:1",
        server.ws_base_url
    ))
    .await
    .expect_err("retained history no longer proves stale cursor");
    assert_eq!(http_error_status(stale), StatusCode::UNPROCESSABLE_ENTITY);

    let future = connect_async(format!(
        "{}/ws/v1alpha1?token=test-token&cursor=v1:3",
        server.ws_base_url
    ))
    .await
    .expect_err("future cursor must reject upgrade");
    assert_eq!(http_error_status(future), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn command_socket_cursor_validation_sanitizes_backend_failure() {
    let store = Arc::new(TestEventStore::default());
    store.fail_on_read(1);
    let server = spawn_server(Config::default(), EventBus::new(4), Some(store)).await;
    let error = connect_async(format!(
        "{}/ws/v1alpha1?token=test-token&cursor=v1:1",
        server.ws_base_url
    ))
    .await
    .expect_err("cursor validation backend failure rejects upgrade");
    let WebSocketError::Http(response) = error else {
        panic!("expected HTTP handshake failure");
    };
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = response
        .body()
        .as_deref()
        .map(String::from_utf8_lossy)
        .unwrap_or_default();
    assert!(body.contains("durable reconnect cursor validation unavailable"));
    assert!(!body.contains(PRIVATE_BACKEND_SENTINEL));
}

#[tokio::test]
async fn command_socket_rejects_the_producer_less_system_channel() {
    let server = spawn_server(
        Config::default(),
        EventBus::new(4),
        Some(Arc::new(TestEventStore::default())),
    )
    .await;
    let mut socket = connect_command(&server).await;
    send_command(
        &mut socket,
        serde_json::json!({
            "msg_type": "subscribe",
            "id": "system-subscribe",
            "channel": "system",
        }),
    )
    .await;
    let error = next_json(&mut socket).await;
    assert_eq!(error["msg_type"], "error");
    assert_eq!(error["id"], "system-subscribe");
    assert_eq!(error["payload"]["reason"], "unknown channel: system");
}

#[tokio::test]
async fn command_socket_recovery_failure_is_explicit_sanitized_and_store_is_required() {
    let run_id = RunId::new();
    let store = Arc::new(TestEventStore::default());
    store.fail_on_read(1);
    let bus = EventBus::new(4);
    let server = spawn_server(Config::default(), bus.clone(), Some(store.clone())).await;
    let mut socket = connect_command(&server).await;
    subscribe(&mut socket, &format!("runs:{run_id}"), "subscribe-run").await;
    let (first_stored, first_live) = stored_event(1, run_id, EventKind::RunCreated);
    store.insert(first_stored).await;
    bus.publish(first_live);
    assert_eq!(next_json(&mut socket).await["payload"]["sequence"], 1);
    let (second_stored, second_live) = stored_event(2, run_id, EventKind::RunStarted);
    store.insert(second_stored).await;
    bus.publish(second_live);

    let protocol_error = next_json(&mut socket).await;
    assert_eq!(protocol_error["msg_type"], "error");
    assert_eq!(
        protocol_error["payload"]["reason"],
        "durable event recovery unavailable"
    );
    assert!(!protocol_error
        .to_string()
        .contains(PRIVATE_BACKEND_SENTINEL));
    let close = socket
        .next()
        .await
        .expect("explicit close message")
        .expect("valid close message");
    let Message::Close(Some(frame)) = close else {
        panic!("expected close frame, got {close:?}");
    };
    assert_eq!(frame.code, CloseCode::Error);
    assert_eq!(frame.reason, "durable event recovery unavailable");
    assert!(!frame.reason.contains(PRIVATE_BACKEND_SENTINEL));

    let missing_store = spawn_server(Config::default(), EventBus::new(4), None).await;
    let unavailable = connect_async(format!(
        "{}/ws/v1alpha1?token=test-token",
        missing_store.ws_base_url
    ))
    .await
    .expect_err("command socket requires durable recovery store");
    assert_eq!(http_error_status(unavailable), StatusCode::NOT_IMPLEMENTED);
}
