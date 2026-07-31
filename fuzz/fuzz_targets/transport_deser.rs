#![no_main]

use libfuzzer_sys::fuzz_target;
use polkagent_transport_trait::{
    AuthenticatedSender, Classification, DeliveryId, IncomingMessage, MessageBody, OutgoingBody,
    OutgoingMessage, SenderTrustTier, UserId,
};

fuzz_target!(|data: &[u8]| {
    // FZ-03: Fuzz all transport message types for JSON deserialization.
    // None of these must ever panic — only return Ok or Err.

    // Deserialize arbitrary bytes as each transport type.
    let _ = serde_json::from_slice::<IncomingMessage>(data);
    let _ = serde_json::from_slice::<OutgoingMessage>(data);
    let _ = serde_json::from_slice::<MessageBody>(data);
    let _ = serde_json::from_slice::<OutgoingBody>(data);
    let _ = serde_json::from_slice::<AuthenticatedSender>(data);
    let _ = serde_json::from_slice::<SenderTrustTier>(data);
    let _ = serde_json::from_slice::<Classification>(data);
    let _ = serde_json::from_slice::<DeliveryId>(data);
    let _ = serde_json::from_slice::<UserId>(data);

    // Also try interpreting as a UTF-8 string and parsing via from_str.
    if let Ok(s) = std::str::from_utf8(data) {
        let _ = serde_json::from_str::<IncomingMessage>(s);
        let _ = serde_json::from_str::<OutgoingMessage>(s);
        let _ = serde_json::from_str::<MessageBody>(s);
        let _ = serde_json::from_str::<OutgoingBody>(s);
        let _ = serde_json::from_str::<SenderTrustTier>(s);
        let _ = serde_json::from_str::<Classification>(s);
    }

    // Round-trip invariant: if we successfully deserialize an IncomingMessage,
    // re-serializing it must also succeed.
    if let Ok(msg) = serde_json::from_slice::<IncomingMessage>(data) {
        let serialized = serde_json::to_string(&msg);
        assert!(
            serialized.is_ok(),
            "serialization of a successfully deserialized IncomingMessage must not fail"
        );
    }

    // Same round-trip check for OutgoingMessage.
    if let Ok(msg) = serde_json::from_slice::<OutgoingMessage>(data) {
        let serialized = serde_json::to_string(&msg);
        assert!(
            serialized.is_ok(),
            "serialization of a successfully deserialized OutgoingMessage must not fail"
        );
    }

    // Verify Classification ordering is stable (no panics in comparison).
    if let Ok(cls) = serde_json::from_slice::<Classification>(data) {
        let _ = cls >= Classification::Public;
        let _ = cls <= Classification::Sensitive;
    }

    // DeliveryId and UserId are just newtypes over String — Display must not panic.
    if let Ok(id) = serde_json::from_slice::<DeliveryId>(data) {
        let _ = id.to_string();
    }
    if let Ok(id) = serde_json::from_slice::<UserId>(data) {
        let _ = id.to_string();
    }
});
