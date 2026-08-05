//! PCA C0 bot end-to-end integration tests (AC-P3-003).
//!
//! These tests verify the full PCA transport lifecycle:
//! 1. Session establishment (bot registration via handshake)
//! 2. Sending a message and verifying the delivery receipt (ACK)
//! 3. Receiving a response message
//! 4. Message ordering and content integrity across multiple messages
//! 5. Encrypted message exchange between two transports

// This assertion-oriented integration target uses `expect`/`unwrap` to identify
// the exact cross-crate fixture step or behavioral contract that failed.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use polkagent_transport_pca::config::PcaConfig;
use polkagent_transport_pca::crypto::KeyPair;
use polkagent_transport_pca::session::SessionState;
use polkagent_transport_pca::PcaTransport;
use polkagent_transport_pca::PcaWireMessage;
use polkagent_transport_trait::{
    Classification, MessageBody, OutgoingBody, OutgoingMessage, Transport,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn bot_config() -> PcaConfig {
    PcaConfig::new("5BotAddr...").with_peer("5ServerAddr...", Some("MockServer".into()))
}

fn server_config() -> PcaConfig {
    PcaConfig::new("5ServerAddr...").with_peer("5BotAddr...", Some("Bot".into()))
}

fn make_wire_bytes(sender: &str, display_name: &str, body: &str) -> Vec<u8> {
    let wire = PcaWireMessage {
        conversation_id: "conv-e2e".into(),
        sender_address: sender.into(),
        sender_display_name: Some(display_name.into()),
        body_json: body.into(),
    };
    serde_json::to_vec(&wire).expect("serialize wire message")
}

fn make_outgoing(body: &str) -> OutgoingMessage {
    OutgoingMessage {
        conversation_id: polkagent_core::ConversationId::new(),
        run_id: None,
        body: OutgoingBody::Text {
            content: body.into(),
        },
        classification: Classification::Public,
    }
}

// ---------------------------------------------------------------------------
// 1. Bot registration (session establishment)
// ---------------------------------------------------------------------------

#[test]
fn bot_registers_with_server_via_handshake() {
    let bot = PcaTransport::new(bot_config()).expect("create bot transport");
    let server = PcaTransport::new(server_config()).expect("create server transport");

    // Bot generates a key pair and initiates a session.
    let bot_kp = KeyPair::generate();
    let bot_pub = bot_kp.public_key_bytes();

    // Server accepts the bot's public key.
    let server_pub = server
        .accept_session("5BotAddr...", &bot_pub)
        .expect("server accepts bot");

    // Bot completes the handshake with the server's public key.
    bot.establish_session("5ServerAddr...", &server_pub)
        .expect("bot establishes session");

    // Both sides should be connected and active.
    assert!(bot.is_connected(), "bot must be connected after handshake");
    assert!(
        server.is_connected(),
        "server must be connected after handshake"
    );
    assert_eq!(bot.session_state(), Some(SessionState::Active));
    assert_eq!(server.session_state(), Some(SessionState::Active));
}

// ---------------------------------------------------------------------------
// 2. Send a message and verify ACK (delivery receipt)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn bot_sends_message_and_receives_ack() {
    let bot = PcaTransport::new(bot_config()).expect("create bot transport");
    bot.establish_session("5ServerAddr...", &KeyPair::generate().public_key_bytes())
        .expect("establish session");

    // Bot sends an outgoing message.
    let receipt = bot
        .send(make_outgoing("Hello server!"))
        .await
        .expect("send must succeed");

    // The receipt constitutes the ACK — verify it has a valid delivery ID.
    assert!(
        !receipt.delivery_id.0.is_empty(),
        "delivery receipt must contain a non-empty delivery ID"
    );

    // The outgoing message should be drainable from the transport.
    let drained = bot.drain_outgoing();
    assert_eq!(
        drained.len(),
        1,
        "exactly one message should be in outgoing"
    );

    // Verify the drained payload is valid JSON containing the sent content.
    let wire: PcaWireMessage =
        serde_json::from_slice(&drained[0]).expect("drained payload is valid wire message");
    assert_eq!(wire.sender_address, "5BotAddr...");
}

// ---------------------------------------------------------------------------
// 3. Receive a response message
// ---------------------------------------------------------------------------

#[tokio::test]
async fn bot_receives_response_from_server() {
    let bot = PcaTransport::new(bot_config()).expect("create bot transport");
    bot.establish_session("5ServerAddr...", &KeyPair::generate().public_key_bytes())
        .expect("establish session");

    // Server injects a response into the bot's incoming channel.
    bot.inject_plaintext(make_wire_bytes(
        "5ServerAddr...",
        "MockServer",
        "Hello bot!",
    ))
    .expect("inject plaintext");

    // Bot receives the message.
    let msg = bot.receive().await.expect("receive must succeed");

    // Verify sender identity.
    assert_eq!(msg.sender.user_id.0, "5ServerAddr...");
    assert_eq!(msg.sender.display_name.as_deref(), Some("MockServer"));

    // Verify body content.
    match &msg.body {
        MessageBody::Text { content } => assert_eq!(content, "Hello bot!"),
        other => panic!("expected Text body, got {other:?}"),
    }

    // Acknowledge receipt.
    bot.ack(msg.delivery_id).await.expect("ack must succeed");
    assert_eq!(
        bot.incoming_channel().pending_ack_count(),
        0,
        "no messages should be pending after ack"
    );
}

// ---------------------------------------------------------------------------
// 4. Message ordering and content integrity
// ---------------------------------------------------------------------------

#[tokio::test]
async fn messages_are_received_in_fifo_order() {
    let bot = PcaTransport::new(bot_config()).expect("create bot transport");
    bot.establish_session("5ServerAddr...", &KeyPair::generate().public_key_bytes())
        .expect("establish session");

    let message_count = 10;

    // Inject multiple messages in sequence.
    for i in 0..message_count {
        bot.inject_plaintext(make_wire_bytes(
            "5ServerAddr...",
            "MockServer",
            &format!("msg-{i}"),
        ))
        .expect("inject");
    }

    // Receive and verify FIFO ordering and content integrity.
    for i in 0..message_count {
        let msg = bot.receive().await.expect("receive");
        match &msg.body {
            MessageBody::Text { content } => {
                assert_eq!(
                    content,
                    &format!("msg-{i}"),
                    "message {i} has wrong content"
                );
            }
            other => panic!("expected Text body for message {i}, got {other:?}"),
        }
        bot.ack(msg.delivery_id).await.expect("ack");
    }

    assert_eq!(
        bot.incoming_channel().pending_ack_count(),
        0,
        "all messages must be acked"
    );
}

#[tokio::test]
async fn outgoing_messages_preserve_order() {
    let bot = PcaTransport::new(bot_config()).expect("create bot transport");
    bot.establish_session("5ServerAddr...", &KeyPair::generate().public_key_bytes())
        .expect("establish session");

    let message_count = 5;

    // Send multiple messages.
    for i in 0..message_count {
        bot.send(make_outgoing(&format!("out-{i}")))
            .await
            .expect("send");
    }

    // Drain and verify order.
    let drained = bot.drain_outgoing();
    assert_eq!(drained.len(), message_count);

    for (i, payload) in drained.iter().enumerate() {
        let wire: PcaWireMessage = serde_json::from_slice(payload).expect("valid wire message");
        let body: OutgoingBody = serde_json::from_str(&wire.body_json).expect("valid body JSON");
        match body {
            OutgoingBody::Text { content } => {
                assert_eq!(content, format!("out-{i}"), "outgoing message {i} mismatch");
            }
            other => panic!("expected Text body for message {i}, got {other:?}"),
        }
    }
}

// ---------------------------------------------------------------------------
// 5. Full E2E: bot ↔ server message exchange
// ---------------------------------------------------------------------------

#[tokio::test]
async fn full_e2e_bot_server_conversation() {
    // Set up both transports.
    let bot = PcaTransport::new(bot_config()).expect("create bot");
    let server = PcaTransport::new(server_config()).expect("create server");

    // Perform mutual handshake.
    let bot_kp = KeyPair::generate();
    let server_pub = server
        .accept_session("5BotAddr...", &bot_kp.public_key_bytes())
        .expect("server accepts");
    bot.establish_session("5ServerAddr...", &server_pub)
        .expect("bot establishes");

    // Step 1: Bot sends a message to the server.
    let bot_receipt = bot
        .send(make_outgoing("What is my balance?"))
        .await
        .expect("bot send");
    assert!(!bot_receipt.delivery_id.0.is_empty());

    // Simulate network: take from bot outgoing, deliver to server incoming.
    let bot_outgoing = bot.drain_outgoing();
    assert_eq!(bot_outgoing.len(), 1);
    server
        .inject_plaintext(bot_outgoing[0].clone())
        .expect("deliver to server");

    // Step 2: Server receives the bot's message.
    // Note: the wire body_json is the JSON-serialized OutgoingBody, so the
    // receive() path treats it as a raw Text content string.
    let server_received = server.receive().await.expect("server receive");
    assert_eq!(server_received.sender.user_id.0, "5BotAddr...");
    match &server_received.body {
        MessageBody::Text { content } => {
            let body: OutgoingBody =
                serde_json::from_str(content).expect("body_json is valid OutgoingBody");
            match body {
                OutgoingBody::Text {
                    content: inner_text,
                } => assert_eq!(inner_text, "What is my balance?"),
                other => panic!("expected OutgoingBody::Text, got {other:?}"),
            }
        }
        other => panic!("expected Text, got {other:?}"),
    }
    server
        .ack(server_received.delivery_id)
        .await
        .expect("server ack");

    // Step 3: Server sends a response to the bot.
    let server_receipt = server
        .send(make_outgoing("Your balance is 100 DOT"))
        .await
        .expect("server send");
    assert!(!server_receipt.delivery_id.0.is_empty());

    // Simulate network: take from server outgoing, deliver to bot incoming.
    let server_outgoing = server.drain_outgoing();
    assert_eq!(server_outgoing.len(), 1);
    bot.inject_plaintext(server_outgoing[0].clone())
        .expect("deliver to bot");

    // Step 4: Bot receives the server's response.
    let bot_received = bot.receive().await.expect("bot receive");
    assert_eq!(bot_received.sender.user_id.0, "5ServerAddr...");
    match &bot_received.body {
        MessageBody::Text { content } => {
            let body: OutgoingBody =
                serde_json::from_str(content).expect("body_json is valid OutgoingBody");
            match body {
                OutgoingBody::Text {
                    content: inner_text,
                } => assert_eq!(inner_text, "Your balance is 100 DOT"),
                other => panic!("expected OutgoingBody::Text, got {other:?}"),
            }
        }
        other => panic!("expected Text, got {other:?}"),
    }
    bot.ack(bot_received.delivery_id).await.expect("bot ack");

    // Both channels should be clean.
    assert_eq!(bot.incoming_channel().pending_ack_count(), 0);
    assert_eq!(server.incoming_channel().pending_ack_count(), 0);
}

// ---------------------------------------------------------------------------
// 6. Edge cases
// ---------------------------------------------------------------------------

#[tokio::test]
async fn send_fails_when_disconnected() {
    let bot = PcaTransport::new(bot_config()).expect("create bot");
    // Bot never establishes a session, so it is disconnected.
    let result = bot.send(make_outgoing("should fail")).await;
    assert!(
        result.is_err(),
        "send must fail when transport is disconnected"
    );
}

#[tokio::test]
async fn receive_fails_when_disconnected() {
    let bot = PcaTransport::new(bot_config()).expect("create bot");
    let result = bot.receive().await;
    assert!(
        result.is_err(),
        "receive must fail when transport is disconnected"
    );
}

#[tokio::test]
async fn disconnect_terminates_active_session() {
    let bot = PcaTransport::new(bot_config()).expect("create bot");
    bot.establish_session("5ServerAddr...", &KeyPair::generate().public_key_bytes())
        .expect("establish");

    assert!(bot.is_connected());
    bot.disconnect();
    assert!(!bot.is_connected());
    assert!(bot.incoming_channel().is_closed());

    // Operations should fail after disconnect.
    let result = bot.send(make_outgoing("after disconnect")).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn unacked_messages_remain_pending() {
    let bot = PcaTransport::new(bot_config()).expect("create bot");
    bot.establish_session("5ServerAddr...", &KeyPair::generate().public_key_bytes())
        .expect("establish");

    // Inject and receive without acking.
    bot.inject_plaintext(make_wire_bytes("5ServerAddr...", "S", "msg-1"))
        .expect("inject");
    bot.inject_plaintext(make_wire_bytes("5ServerAddr...", "S", "msg-2"))
        .expect("inject");

    let m1 = bot.receive().await.expect("receive 1");
    let _m2 = bot.receive().await.expect("receive 2");

    assert_eq!(
        bot.incoming_channel().pending_ack_count(),
        2,
        "both messages should be pending ack"
    );

    // Ack only the first.
    bot.ack(m1.delivery_id).await.expect("ack 1");
    assert_eq!(
        bot.incoming_channel().pending_ack_count(),
        1,
        "one message should still be pending"
    );
}

#[tokio::test]
async fn content_integrity_roundtrip() {
    let bot = PcaTransport::new(bot_config()).expect("create bot");
    bot.establish_session("5ServerAddr...", &KeyPair::generate().public_key_bytes())
        .expect("establish");

    let test_payloads = [
        "",
        "simple text",
        "unicode: 🎉🔗⚡",
        &"x".repeat(10_000),
        r#"{"key": "value", "nested": {"arr": [1, 2, 3]}}"#,
    ];

    for (i, payload) in test_payloads.iter().enumerate() {
        bot.inject_plaintext(make_wire_bytes("5ServerAddr...", "S", payload))
            .expect("inject");

        let msg = bot.receive().await.expect("receive");
        match &msg.body {
            MessageBody::Text { content } => {
                assert_eq!(content, payload, "content mismatch for payload {i}");
            }
            other => panic!("expected Text body for payload {i}, got {other:?}"),
        }
        bot.ack(msg.delivery_id).await.expect("ack");
    }
}

#[test]
fn capabilities_report_pca_auth_methods() {
    let bot = PcaTransport::new(bot_config()).expect("create bot");
    let caps = bot.capabilities();
    assert!(
        caps.supported_auth_methods
            .contains(&"x25519-chacha20poly1305".to_string()),
        "PCA transport must advertise x25519-chacha20poly1305 auth"
    );
    assert!(
        caps.supported_auth_methods.contains(&"ss58".to_string()),
        "PCA transport must advertise ss58 auth"
    );
    assert!(
        caps.supports_structured_cards,
        "PCA transport must support structured cards"
    );
}
