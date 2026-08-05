#![allow(
    clippy::expect_used,
    reason = "the TCP integration target intentionally fails fast on fixture and network setup failures"
)]

use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener as StdTcpListener};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use polkagent_core::{ConversationId, RunId};
use polkagent_transport_pca::network::{
    PcaCancellationFrame, PcaErrorReplyFrame, PcaReplyStatus, PcaStatusReplyFrame, TcpPcaConfig,
    TcpPcaTransport, TcpPeer, PCA_CANCELLATION_COMMAND, PCA_ERROR_COMMAND, PCA_STATUS_COMMAND,
};
use polkagent_transport_pca::PcaWireMessage;
use polkagent_transport_trait::{
    Classification, MessageBody, OutgoingBody, OutgoingMessage, Transport, TransportError,
};
use tempfile::TempDir;

fn loopback_any() -> SocketAddr {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0)
}

fn reserve_loopback_addr() -> SocketAddr {
    let listener = StdTcpListener::bind(loopback_any()).expect("reserve loopback address");
    listener.local_addr().expect("reserved address")
}

fn config(identity: &str, listen_addr: SocketAddr, state_path: &Path) -> TcpPcaConfig {
    TcpPcaConfig::new(identity, listen_addr, state_path)
        .with_retry_interval(Duration::from_millis(25))
        .with_io_timeout(Duration::from_millis(500))
        .with_application_lease_timeout(Duration::from_millis(100))
}

fn outgoing(conversation_id: ConversationId, content: &str) -> OutgoingMessage {
    OutgoingMessage {
        conversation_id,
        run_id: None,
        body: OutgoingBody::Text {
            content: content.into(),
        },
        classification: Classification::Public,
    }
}

async fn wait_for_outbox_empty(transport: &TcpPcaTransport) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if transport.pending_outbox_count().await == 0 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("outbox should receive a durable peer ACK");
}

fn structured_payload<T: serde::de::DeserializeOwned>(
    body: &MessageBody,
    expected_command: &str,
) -> T {
    let MessageBody::StructuredCommand {
        command_type,
        payload_json,
    } = body
    else {
        panic!("expected structured command, got {body:?}");
    };
    assert_eq!(command_type, expected_command);
    serde_json::from_str(payload_json).expect("decode structured PCA frame")
}

#[tokio::test]
async fn tcp_delivery_has_distinct_network_and_application_ack_boundaries() {
    let temp = TempDir::new().expect("temp dir");
    let receiver = TcpPcaTransport::bind(config(
        "5Receiver",
        loopback_any(),
        &temp.path().join("receiver.json"),
    ))
    .await
    .expect("bind receiver");
    let sender = TcpPcaTransport::bind(
        config("5Sender", loopback_any(), &temp.path().join("sender.json"))
            .with_peer(TcpPeer::new(receiver.listen_addr(), "5Receiver")),
    )
    .await
    .expect("bind sender");

    let conversation_id = ConversationId::new();
    sender
        .send(outgoing(conversation_id, "across a real socket"))
        .await
        .expect("durably enqueue outbound message");
    let delivered_message = tokio::time::timeout(Duration::from_secs(5), receiver.receive())
        .await
        .expect("network delivery timeout")
        .expect("receive network message");

    assert_eq!(delivered_message.conversation_id, conversation_id);
    assert_eq!(delivered_message.sender.user_id.0, "5Sender");
    let MessageBody::Text { content } = &delivered_message.body else {
        panic!("expected text body");
    };
    assert_eq!(content, "across a real socket");

    // The sender's durable outbox clears after the receiver has persisted the
    // message, but the receiver retains it until the application-level ACK.
    wait_for_outbox_empty(&sender).await;
    assert_eq!(receiver.pending_inbox_count().await, 1);
    receiver
        .ack(delivered_message.delivery_id)
        .await
        .expect("application ACK");
    assert_eq!(receiver.pending_inbox_count().await, 0);
    assert_eq!(receiver.dedup_marker_count().await, 1);

    sender.shutdown().await;
    receiver.shutdown().await;
}

#[tokio::test]
async fn sender_reconnects_and_retries_durable_outbox_when_peer_appears() {
    let temp = TempDir::new().expect("temp dir");
    let receiver_addr = reserve_loopback_addr();
    let sender = TcpPcaTransport::bind(
        config("5Sender", loopback_any(), &temp.path().join("sender.json"))
            .with_peer(TcpPeer::new(receiver_addr, "5Receiver")),
    )
    .await
    .expect("bind sender");
    sender
        .send(outgoing(ConversationId::new(), "queued while offline"))
        .await
        .expect("persist while peer offline");
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(sender.pending_outbox_count().await, 1);

    let receiver = TcpPcaTransport::bind(config(
        "5Receiver",
        receiver_addr,
        &temp.path().join("receiver.json"),
    ))
    .await
    .expect("bind receiver after sender");
    let delivered_message = tokio::time::timeout(Duration::from_secs(5), receiver.receive())
        .await
        .expect("reconnect timeout")
        .expect("receive after reconnect");
    wait_for_outbox_empty(&sender).await;
    receiver
        .ack(delivered_message.delivery_id)
        .await
        .expect("ack");

    sender.shutdown().await;
    receiver.shutdown().await;
}

#[tokio::test]
async fn unacked_inbox_redelivers_with_same_id_after_receiver_restart() {
    let temp = TempDir::new().expect("temp dir");
    let receiver_path = temp.path().join("receiver.json");
    let receiver = TcpPcaTransport::bind(config("5Receiver", loopback_any(), &receiver_path))
        .await
        .expect("bind receiver");
    let receiver_addr = receiver.listen_addr();
    let sender = TcpPcaTransport::bind(
        config("5Sender", loopback_any(), &temp.path().join("sender.json"))
            .with_peer(TcpPeer::new(receiver_addr, "5Receiver")),
    )
    .await
    .expect("bind sender");
    sender
        .send(outgoing(ConversationId::new(), "survive receiver crash"))
        .await
        .expect("send");
    let first = tokio::time::timeout(Duration::from_secs(5), receiver.receive())
        .await
        .expect("first delivery timeout")
        .expect("first delivery");
    wait_for_outbox_empty(&sender).await;

    // Deliberately omit the application ACK before stopping the receiver.
    receiver.shutdown().await;
    drop(receiver);
    let restarted = TcpPcaTransport::bind(config("5Receiver", receiver_addr, &receiver_path))
        .await
        .expect("restart receiver on same socket and state");
    let redelivered = tokio::time::timeout(Duration::from_secs(2), restarted.receive())
        .await
        .expect("restart redelivery timeout")
        .expect("restart redelivery");
    assert_eq!(redelivered.delivery_id, first.delivery_id);
    restarted
        .ack(redelivered.delivery_id)
        .await
        .expect("ack recovered delivery");

    sender.shutdown().await;
    restarted.shutdown().await;
}

#[tokio::test]
async fn abandoned_application_lease_redelivers_without_process_restart() {
    let temp = TempDir::new().expect("temp dir");
    let receiver = TcpPcaTransport::bind(config(
        "5Receiver",
        loopback_any(),
        &temp.path().join("receiver.json"),
    ))
    .await
    .expect("bind receiver");
    let sender = TcpPcaTransport::bind(
        config("5Sender", loopback_any(), &temp.path().join("sender.json"))
            .with_peer(TcpPeer::new(receiver.listen_addr(), "5Receiver")),
    )
    .await
    .expect("bind sender");
    sender
        .send(outgoing(ConversationId::new(), "lease me"))
        .await
        .expect("send");
    let first = tokio::time::timeout(Duration::from_secs(5), receiver.receive())
        .await
        .expect("first lease timeout")
        .expect("first lease");

    // Do not ACK. Once the in-memory lease expires, the same persisted
    // delivery becomes available again without restarting the transport.
    let second = tokio::time::timeout(Duration::from_secs(1), receiver.receive())
        .await
        .expect("lease redelivery timeout")
        .expect("lease redelivery");
    assert_eq!(second.delivery_id, first.delivery_id);
    receiver.ack(second.delivery_id).await.expect("ack retry");

    sender.shutdown().await;
    receiver.shutdown().await;
}

#[tokio::test]
async fn unacknowledged_head_delivery_blocks_later_messages() {
    let temp = TempDir::new().expect("temp dir");
    let receiver = TcpPcaTransport::bind(
        config(
            "5Receiver",
            loopback_any(),
            &temp.path().join("receiver.json"),
        )
        .with_application_lease_timeout(Duration::from_secs(1)),
    )
    .await
    .expect("bind receiver");
    let sender = TcpPcaTransport::bind(
        config("5Sender", loopback_any(), &temp.path().join("sender.json"))
            .with_peer(TcpPeer::new(receiver.listen_addr(), "5Receiver")),
    )
    .await
    .expect("bind sender");
    let conversation_id = ConversationId::new();
    sender
        .send(outgoing(conversation_id, "first"))
        .await
        .expect("send first");
    sender
        .send(outgoing(conversation_id, "second"))
        .await
        .expect("send second");

    let first = tokio::time::timeout(Duration::from_secs(5), receiver.receive())
        .await
        .expect("first receive timeout")
        .expect("first receive");
    assert!(matches!(&first.body, MessageBody::Text { content } if content == "first"));
    let early_second = tokio::time::timeout(Duration::from_millis(50), receiver.receive()).await;
    assert!(
        early_second.is_err(),
        "later message must wait for head ACK"
    );
    receiver.ack(first.delivery_id).await.expect("ack first");
    let second = tokio::time::timeout(Duration::from_secs(1), receiver.receive())
        .await
        .expect("second receive timeout")
        .expect("second receive");
    assert!(matches!(&second.body, MessageBody::Text { content } if content == "second"));
    receiver.ack(second.delivery_id).await.expect("ack second");

    sender.shutdown().await;
    receiver.shutdown().await;
}

#[tokio::test]
async fn duplicate_delivery_is_wire_acked_without_second_application_message() {
    let temp = TempDir::new().expect("temp dir");
    let receiver = TcpPcaTransport::bind(config(
        "5Receiver",
        loopback_any(),
        &temp.path().join("receiver.json"),
    ))
    .await
    .expect("bind receiver");
    let sender = TcpPcaTransport::bind(
        config("5Sender", loopback_any(), &temp.path().join("sender.json"))
            .with_peer(TcpPeer::new(receiver.listen_addr(), "5Receiver")),
    )
    .await
    .expect("bind sender");
    let conversation_id = ConversationId::new();
    let receipt = sender
        .send(outgoing(conversation_id, "only once"))
        .await
        .expect("send first copy");
    let delivered_message = tokio::time::timeout(Duration::from_secs(5), receiver.receive())
        .await
        .expect("first copy timeout")
        .expect("receive first copy");
    wait_for_outbox_empty(&sender).await;
    sender.shutdown().await;

    // Recreate the durable sender outbox as it would look after a crash that
    // occurred before the wire ACK was committed locally. The same delivery
    // ID is retried over a new TCP connection and a fresh encrypted session.
    let wire = PcaWireMessage {
        conversation_id: conversation_id.to_string(),
        sender_address: "5Sender".into(),
        sender_display_name: None,
        body_json: serde_json::to_string(&OutgoingBody::Text {
            content: "only once".into(),
        })
        .expect("serialize body"),
    };
    let retry_state = serde_json::json!({
        "version": 1,
        "inbox": [],
        "outbox": [{
            "delivery_id": receipt.delivery_id.0,
            "payload": serde_json::to_vec(&wire).expect("serialize retry wire message"),
        }],
        "dedup_markers": [],
    });
    let retry_path = temp.path().join("retry-sender.json");
    std::fs::write(
        &retry_path,
        serde_json::to_vec_pretty(&retry_state).expect("serialize retry state"),
    )
    .expect("write retry state");
    let retry_sender = TcpPcaTransport::bind(
        config("5Sender", loopback_any(), &retry_path)
            .with_peer(TcpPeer::new(receiver.listen_addr(), "5Receiver")),
    )
    .await
    .expect("restart sender with pre-ACK outbox");
    wait_for_outbox_empty(&retry_sender).await;

    assert_eq!(receiver.pending_inbox_count().await, 1);
    assert_eq!(receiver.dedup_marker_count().await, 1);
    receiver
        .ack(delivered_message.delivery_id)
        .await
        .expect("ack original application delivery");
    let duplicate = tokio::time::timeout(Duration::from_millis(150), receiver.receive()).await;
    assert!(
        duplicate.is_err(),
        "duplicate must not reach the application"
    );

    retry_sender.shutdown().await;
    receiver.shutdown().await;
}

#[tokio::test]
async fn cancellation_reconnects_from_durable_outbox_and_deduplicates_retry() {
    let temp = TempDir::new().expect("temp dir");
    let receiver_addr = reserve_loopback_addr();
    let sender_path = temp.path().join("cancel-sender.json");
    let sender = TcpPcaTransport::bind(
        config("5Sender", loopback_any(), &sender_path)
            .with_peer(TcpPeer::new(receiver_addr, "5Receiver")),
    )
    .await
    .expect("bind offline cancellation sender");
    let conversation_id = ConversationId::new();
    let run_id = RunId::new();
    let cancellation =
        PcaCancellationFrame::run(conversation_id, run_id, Some("user requested stop".into()));
    let receipt = sender
        .send_cancellation(cancellation.clone())
        .await
        .expect("persist cancellation while peer offline");
    tokio::time::sleep(Duration::from_millis(75)).await;
    assert_eq!(sender.pending_outbox_count().await, 1);
    sender.shutdown().await;
    let pre_ack_state = std::fs::read(&sender_path).expect("read pre-ACK cancellation state");

    let receiver = TcpPcaTransport::bind(config(
        "5Receiver",
        receiver_addr,
        &temp.path().join("cancel-receiver.json"),
    ))
    .await
    .expect("bind cancellation receiver");
    let restarted_sender = TcpPcaTransport::bind(
        config("5Sender", loopback_any(), &sender_path)
            .with_peer(TcpPeer::new(receiver_addr, "5Receiver")),
    )
    .await
    .expect("restart cancellation sender");
    let delivered_message = tokio::time::timeout(Duration::from_secs(5), receiver.receive())
        .await
        .expect("cancellation receive timeout")
        .expect("receive cancellation");
    assert_eq!(delivered_message.delivery_id, receipt.delivery_id);
    assert_eq!(
        structured_payload::<PcaCancellationFrame>(
            &delivered_message.body,
            PCA_CANCELLATION_COMMAND
        ),
        cancellation
    );
    wait_for_outbox_empty(&restarted_sender).await;
    restarted_sender.shutdown().await;

    // Restore the sender's exact pre-wire-ACK state. The duplicate traverses
    // a fresh connection but is ACKed from the receiver's durable dedup marker.
    std::fs::write(&sender_path, pre_ack_state).expect("restore pre-ACK sender state");
    let duplicate_sender = TcpPcaTransport::bind(
        config("5Sender", loopback_any(), &sender_path)
            .with_peer(TcpPeer::new(receiver_addr, "5Receiver")),
    )
    .await
    .expect("restart duplicate cancellation sender");
    wait_for_outbox_empty(&duplicate_sender).await;
    assert_eq!(receiver.pending_inbox_count().await, 1);
    assert_eq!(receiver.dedup_marker_count().await, 1);
    receiver
        .ack(delivered_message.delivery_id)
        .await
        .expect("ack cancel");

    duplicate_sender.shutdown().await;
    receiver.shutdown().await;
}

#[tokio::test]
async fn structured_status_survives_receiver_restart_with_same_delivery_id() {
    let temp = TempDir::new().expect("temp dir");
    let receiver_path = temp.path().join("status-receiver.json");
    let receiver = TcpPcaTransport::bind(config("5Receiver", loopback_any(), &receiver_path))
        .await
        .expect("bind status receiver");
    let receiver_addr = receiver.listen_addr();
    let sender = TcpPcaTransport::bind(
        config(
            "5Sender",
            loopback_any(),
            &temp.path().join("status-sender.json"),
        )
        .with_peer(TcpPeer::new(receiver_addr, "5Receiver")),
    )
    .await
    .expect("bind status sender");
    let mut status = PcaStatusReplyFrame::new(
        ConversationId::new(),
        Some(RunId::new()),
        PcaReplyStatus::Running,
    );
    status.message = Some("executing tools".into());
    status.progress_percent = Some(40);
    status.metadata_json = Some(r#"{"step":2}"#.into());
    sender
        .send_status_reply(status.clone())
        .await
        .expect("send status");
    let first = tokio::time::timeout(Duration::from_secs(5), receiver.receive())
        .await
        .expect("status timeout")
        .expect("receive status");
    assert_eq!(
        structured_payload::<PcaStatusReplyFrame>(&first.body, PCA_STATUS_COMMAND),
        status
    );
    wait_for_outbox_empty(&sender).await;

    receiver.shutdown().await;
    drop(receiver);
    let restarted = TcpPcaTransport::bind(config("5Receiver", receiver_addr, &receiver_path))
        .await
        .expect("restart status receiver");
    let recovered = tokio::time::timeout(Duration::from_secs(2), restarted.receive())
        .await
        .expect("status recovery timeout")
        .expect("recover status");
    assert_eq!(recovered.delivery_id, first.delivery_id);
    assert_eq!(
        structured_payload::<PcaStatusReplyFrame>(&recovered.body, PCA_STATUS_COMMAND),
        status
    );
    restarted
        .ack(recovered.delivery_id)
        .await
        .expect("ack status");

    sender.shutdown().await;
    restarted.shutdown().await;
}

#[tokio::test]
async fn control_frames_enforce_fields_total_size_and_shared_backpressure() {
    let temp = TempDir::new().expect("temp dir");
    let transport = TcpPcaTransport::bind(
        config(
            "5Sender",
            loopback_any(),
            &temp.path().join("control-limits.json"),
        )
        .with_peer(TcpPeer::new(reserve_loopback_addr(), "5Offline"))
        .with_max_in_flight(1)
        .with_max_message_bytes(512)
        .with_max_control_field_bytes(32),
    )
    .await
    .expect("bind limited control sender");
    let conversation_id = ConversationId::new();

    let too_long = PcaCancellationFrame::active_conversation(conversation_id, Some("x".repeat(33)));
    assert!(matches!(
        transport.send_cancellation(too_long).await,
        Err(TransportError::DeliveryFailed { .. })
    ));

    let mut invalid_status =
        PcaStatusReplyFrame::new(conversation_id, None, PcaReplyStatus::Running);
    invalid_status.progress_percent = Some(101);
    assert!(matches!(
        transport.send_status_reply(invalid_status).await,
        Err(TransportError::DeliveryFailed { .. })
    ));
    let mut invalid_metadata =
        PcaStatusReplyFrame::new(conversation_id, None, PcaReplyStatus::Running);
    invalid_metadata.metadata_json = Some("not-json".into());
    assert!(matches!(
        transport.send_status_reply(invalid_metadata).await,
        Err(TransportError::DeliveryFailed { .. })
    ));

    let invalid_code = PcaErrorReplyFrame::new(conversation_id, None, "bad-code", "failed");
    assert!(matches!(
        transport.send_error_reply(invalid_code).await,
        Err(TransportError::DeliveryFailed { .. })
    ));
    let mut invalid_retry =
        PcaErrorReplyFrame::new(conversation_id, None, "PROVIDER_DOWN", "failed");
    invalid_retry.retry_after_ms = Some(500);
    assert!(matches!(
        transport.send_error_reply(invalid_retry).await,
        Err(TransportError::DeliveryFailed { .. })
    ));
    assert_eq!(transport.pending_outbox_count().await, 0);

    transport
        .send_cancellation(PcaCancellationFrame::active_conversation(
            conversation_id,
            Some("stop".into()),
        ))
        .await
        .expect("valid cancellation occupies offline outbox");
    let full = transport
        .send_status_reply(PcaStatusReplyFrame::new(
            conversation_id,
            None,
            PcaReplyStatus::Cancelled,
        ))
        .await
        .expect_err("control frames share bounded outbox");
    assert!(matches!(full, TransportError::RateLimit { .. }));
    transport.shutdown().await;

    let total_limited = TcpPcaTransport::bind(
        config(
            "5Sender",
            loopback_any(),
            &temp.path().join("total-limit.json"),
        )
        .with_max_message_bytes(256)
        .with_max_control_field_bytes(128),
    )
    .await
    .expect("bind total-size sender");
    let mut large_status = PcaStatusReplyFrame::new(conversation_id, None, PcaReplyStatus::Running);
    large_status.metadata_json = Some(format!(r#"{{"value":"{}"}}"#, "x".repeat(100)));
    assert!(matches!(
        total_limited.send_status_reply(large_status).await,
        Err(TransportError::DeliveryFailed { .. })
    ));
    assert_eq!(total_limited.pending_outbox_count().await, 0);
    total_limited.shutdown().await;
}

#[tokio::test]
async fn receiver_rejects_invalid_persisted_control_frame_without_wire_ack() {
    let temp = TempDir::new().expect("temp dir");
    let receiver_addr = reserve_loopback_addr();
    let sender_path = temp.path().join("invalid-control-sender.json");
    let sender = TcpPcaTransport::bind(
        config("5Sender", loopback_any(), &sender_path)
            .with_peer(TcpPeer::new(receiver_addr, "5Receiver")),
    )
    .await
    .expect("bind offline sender");
    sender
        .send_error_reply(PcaErrorReplyFrame::new(
            ConversationId::new(),
            None,
            "VALID_CODE",
            "valid before state corruption",
        ))
        .await
        .expect("persist valid error reply");
    sender.shutdown().await;

    // Model a corrupted or malicious durable producer by changing the opaque
    // encrypted-application payload after sender-side validation.
    let mut state: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&sender_path).expect("read sender state"))
            .expect("decode sender state");
    let payload: Vec<u8> = serde_json::from_value(state["outbox"][0]["payload"].clone())
        .expect("decode durable payload bytes");
    let mut application: serde_json::Value =
        serde_json::from_slice(&payload).expect("decode application frame");
    application["frame"]["code"] = serde_json::Value::String("invalid-code".into());
    state["outbox"][0]["payload"] = serde_json::to_value(
        serde_json::to_vec(&application).expect("encode corrupt application frame"),
    )
    .expect("encode corrupt payload bytes");
    std::fs::write(
        &sender_path,
        serde_json::to_vec_pretty(&state).expect("encode corrupt sender state"),
    )
    .expect("write corrupt sender state");

    let receiver = TcpPcaTransport::bind(config(
        "5Receiver",
        receiver_addr,
        &temp.path().join("invalid-control-receiver.json"),
    ))
    .await
    .expect("bind strict receiver");
    let restarted_sender = TcpPcaTransport::bind(
        config("5Sender", loopback_any(), &sender_path)
            .with_peer(TcpPeer::new(receiver_addr, "5Receiver")),
    )
    .await
    .expect("restart corrupted sender");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(receiver.pending_inbox_count().await, 0);
    assert_eq!(receiver.dedup_marker_count().await, 0);
    assert_eq!(restarted_sender.pending_outbox_count().await, 1);

    restarted_sender.shutdown().await;
    receiver.shutdown().await;
}

#[tokio::test]
async fn offline_outbox_enforces_backpressure_and_message_limit() {
    let temp = TempDir::new().expect("temp dir");
    let transport = TcpPcaTransport::bind(
        config("5Sender", loopback_any(), &temp.path().join("sender.json"))
            .with_peer(TcpPeer::new(reserve_loopback_addr(), "5Offline"))
            .with_max_in_flight(1)
            .with_max_message_bytes(256),
    )
    .await
    .expect("bind sender");

    transport
        .send(outgoing(ConversationId::new(), "first"))
        .await
        .expect("first message fits durable outbox");
    let full = transport
        .send(outgoing(ConversationId::new(), "second"))
        .await
        .expect_err("second message must be backpressured");
    assert!(matches!(full, TransportError::RateLimit { .. }));

    transport.shutdown().await;

    let oversized = TcpPcaTransport::bind(
        config(
            "5Sender",
            loopback_any(),
            &temp.path().join("oversized.json"),
        )
        .with_max_message_bytes(128),
    )
    .await
    .expect("bind small transport");
    let error = oversized
        .send(outgoing(ConversationId::new(), &"x".repeat(500)))
        .await
        .expect_err("oversized message must fail before persistence");
    assert!(matches!(error, TransportError::DeliveryFailed { .. }));
    assert_eq!(oversized.pending_outbox_count().await, 0);
    oversized.shutdown().await;
}

#[tokio::test]
async fn dropping_transport_releases_listener_even_with_idle_peer() {
    let temp = TempDir::new().expect("temp dir");
    let transport = TcpPcaTransport::bind(config(
        "5Receiver",
        loopback_any(),
        &temp.path().join("receiver.json"),
    ))
    .await
    .expect("bind transport");
    let listen_addr = transport.listen_addr();
    let _idle_peer = tokio::net::TcpStream::connect(listen_addr)
        .await
        .expect("connect idle peer");
    tokio::time::sleep(Duration::from_millis(20)).await;

    // Background tasks hold only weak transport references while waiting, so
    // callers are not required to invoke `shutdown` to break an Arc cycle.
    drop(transport);
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            match tokio::net::TcpListener::bind(listen_addr).await {
                Ok(listener) => {
                    drop(listener);
                    return;
                }
                Err(_) => tokio::time::sleep(Duration::from_millis(10)).await,
            }
        }
    })
    .await
    .expect("listener address should be released after drop");
}

/// This ignored test is launched by `tcp_transport_works_across_processes` in
/// a fresh OS process. Keeping the helper in the integration-test binary avoids
/// adding a production executable solely for test orchestration.
#[tokio::test]
#[ignore = "subprocess helper launched explicitly by its parent integration test"]
async fn child_process_receives_one_message() {
    let Ok(listen_addr) = std::env::var("PCA_CHILD_LISTEN") else {
        return;
    };
    let state_path = std::env::var("PCA_CHILD_STATE").expect("child state path");
    let ready_path = std::env::var("PCA_CHILD_READY").expect("child ready path");
    let result_path = std::env::var("PCA_CHILD_RESULT").expect("child result path");
    let transport = TcpPcaTransport::bind(config(
        "5Child",
        listen_addr.parse().expect("child listen address"),
        Path::new(&state_path),
    ))
    .await
    .expect("bind child transport");
    std::fs::write(&ready_path, b"ready").expect("write child ready marker");
    let message = tokio::time::timeout(Duration::from_secs(10), transport.receive())
        .await
        .expect("child receive timeout")
        .expect("child receive");
    let MessageBody::Text { content } = message.body else {
        panic!("expected child text body");
    };
    std::fs::write(&result_path, content).expect("write child result");
    transport.ack(message.delivery_id).await.expect("child ack");
    transport.shutdown().await;
}

/// Subprocess helper for typed status/error interoperability coverage.
#[tokio::test]
#[ignore = "subprocess helper launched explicitly by its parent integration test"]
async fn child_process_receives_control_replies() {
    let Ok(listen_addr) = std::env::var("PCA_CONTROL_CHILD_LISTEN") else {
        return;
    };
    let state_path = std::env::var("PCA_CONTROL_CHILD_STATE").expect("child state path");
    let ready_path = std::env::var("PCA_CONTROL_CHILD_READY").expect("child ready path");
    let result_path = std::env::var("PCA_CONTROL_CHILD_RESULT").expect("child result path");
    let transport = TcpPcaTransport::bind(config(
        "5ControlChild",
        listen_addr.parse().expect("child listen address"),
        Path::new(&state_path),
    ))
    .await
    .expect("bind control child transport");
    std::fs::write(&ready_path, b"ready").expect("write child ready marker");

    let mut captured = Vec::new();
    for _ in 0..2 {
        let message = tokio::time::timeout(Duration::from_secs(10), transport.receive())
            .await
            .expect("child control receive timeout")
            .expect("child control receive");
        let MessageBody::StructuredCommand {
            command_type,
            payload_json,
        } = message.body
        else {
            panic!("expected child structured command");
        };
        captured.push(serde_json::json!({
            "command_type": command_type,
            "payload": serde_json::from_str::<serde_json::Value>(&payload_json)
                .expect("decode child control payload"),
        }));
        transport.ack(message.delivery_id).await.expect("child ack");
    }
    std::fs::write(
        &result_path,
        serde_json::to_vec_pretty(&captured).expect("encode child control result"),
    )
    .expect("write child control result");
    transport.shutdown().await;
}

#[tokio::test]
async fn status_and_error_replies_cross_process_boundary() {
    let temp = TempDir::new().expect("temp dir");
    let child_addr = reserve_loopback_addr();
    let ready_path = temp.path().join("control-ready");
    let result_path = temp.path().join("control-result.json");
    let child_state = temp.path().join("control-child-state.json");
    let mut child = Command::new(std::env::current_exe().expect("current test executable"))
        .args([
            "--exact",
            "child_process_receives_control_replies",
            "--ignored",
            "--nocapture",
        ])
        .env("PCA_CONTROL_CHILD_LISTEN", child_addr.to_string())
        .env("PCA_CONTROL_CHILD_STATE", &child_state)
        .env("PCA_CONTROL_CHILD_READY", &ready_path)
        .env("PCA_CONTROL_CHILD_RESULT", &result_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn PCA control child process");

    tokio::time::timeout(Duration::from_secs(5), async {
        while !ready_path.exists() {
            if let Some(status) = child.try_wait().expect("poll control child") {
                panic!("control child exited before readiness: {status}");
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("control child readiness timeout");

    let sender = TcpPcaTransport::bind(
        config(
            "5ControlParent",
            loopback_any(),
            &temp.path().join("control-parent-state.json"),
        )
        .with_peer(TcpPeer::new(child_addr, "5ControlChild")),
    )
    .await
    .expect("bind control parent");
    let conversation_id = ConversationId::new();
    let run_id = RunId::new();
    let mut status = PcaStatusReplyFrame::new(
        conversation_id,
        Some(run_id),
        PcaReplyStatus::WaitingForApproval,
    );
    status.message = Some("approval required".into());
    status.metadata_json = Some(r#"{"approval_id":"a-1"}"#.into());
    sender
        .send_status_reply(status.clone())
        .await
        .expect("send cross-process status");
    let mut error = PcaErrorReplyFrame::new(
        conversation_id,
        Some(run_id),
        "PROVIDER_UNAVAILABLE",
        "provider is temporarily unavailable",
    );
    error.retryable = true;
    error.retry_after_ms = Some(250);
    error.details_json = Some(r#"{"provider":"test"}"#.into());
    sender
        .send_error_reply(error.clone())
        .await
        .expect("send cross-process error");
    wait_for_outbox_empty(&sender).await;

    let child_status = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(status) = child.try_wait().expect("poll control child exit") {
                return status;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("control child exit timeout");
    assert!(
        child_status.success(),
        "control child failed: {child_status}"
    );
    let captured: Vec<serde_json::Value> =
        serde_json::from_slice(&std::fs::read(&result_path).expect("read child control result"))
            .expect("decode child control result");
    assert_eq!(captured.len(), 2);
    assert_eq!(captured[0]["command_type"], PCA_STATUS_COMMAND);
    assert_eq!(captured[1]["command_type"], PCA_ERROR_COMMAND);
    assert_eq!(
        serde_json::from_value::<PcaStatusReplyFrame>(captured[0]["payload"].clone())
            .expect("decode captured status"),
        status
    );
    assert_eq!(
        serde_json::from_value::<PcaErrorReplyFrame>(captured[1]["payload"].clone())
            .expect("decode captured error"),
        error
    );
    sender.shutdown().await;
}

#[tokio::test]
async fn tcp_transport_works_across_processes() {
    let temp = TempDir::new().expect("temp dir");
    let child_addr = reserve_loopback_addr();
    let ready_path = temp.path().join("ready");
    let result_path = temp.path().join("result");
    let child_state = temp.path().join("child-state.json");
    let mut child = Command::new(std::env::current_exe().expect("current test executable"))
        .args([
            "--exact",
            "child_process_receives_one_message",
            "--ignored",
            "--nocapture",
        ])
        .env("PCA_CHILD_LISTEN", child_addr.to_string())
        .env("PCA_CHILD_STATE", &child_state)
        .env("PCA_CHILD_READY", &ready_path)
        .env("PCA_CHILD_RESULT", &result_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn PCA child process");

    tokio::time::timeout(Duration::from_secs(5), async {
        while !ready_path.exists() {
            if let Some(status) = child.try_wait().expect("poll child") {
                panic!("child exited before readiness: {status}");
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("child readiness timeout");

    let sender = TcpPcaTransport::bind(
        config(
            "5Parent",
            loopback_any(),
            &temp.path().join("parent-state.json"),
        )
        .with_peer(TcpPeer::new(child_addr, "5Child")),
    )
    .await
    .expect("bind parent sender");
    sender
        .send(outgoing(ConversationId::new(), "cross-process payload"))
        .await
        .expect("send to child process");
    wait_for_outbox_empty(&sender).await;

    let status = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(status) = child.try_wait().expect("poll child exit") {
                return status;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("child exit timeout");
    assert!(status.success(), "child process failed: {status}");
    assert_eq!(
        std::fs::read_to_string(&result_path).expect("read child result"),
        "cross-process payload"
    );
    sender.shutdown().await;
}
