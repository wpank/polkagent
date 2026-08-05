use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener as StdTcpListener};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use polkagent_core::ConversationId;
use polkagent_transport_pca::network::{TcpPcaConfig, TcpPcaTransport, TcpPeer};
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
    let received = tokio::time::timeout(Duration::from_secs(5), receiver.receive())
        .await
        .expect("network delivery timeout")
        .expect("receive network message");

    assert_eq!(received.conversation_id, conversation_id);
    assert_eq!(received.sender.user_id.0, "5Sender");
    let MessageBody::Text { content } = &received.body else {
        panic!("expected text body");
    };
    assert_eq!(content, "across a real socket");

    // The sender's durable outbox clears after the receiver has persisted the
    // message, but the receiver retains it until the application-level ACK.
    wait_for_outbox_empty(&sender).await;
    assert_eq!(receiver.pending_inbox_count().await, 1);
    receiver
        .ack(received.delivery_id)
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
    let received = tokio::time::timeout(Duration::from_secs(5), receiver.receive())
        .await
        .expect("reconnect timeout")
        .expect("receive after reconnect");
    wait_for_outbox_empty(&sender).await;
    receiver.ack(received.delivery_id).await.expect("ack");

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
    let received = tokio::time::timeout(Duration::from_secs(5), receiver.receive())
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
        .ack(received.delivery_id)
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
#[ignore]
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
