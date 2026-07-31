#![no_main]

use libfuzzer_sys::fuzz_target;
use polkagent_core::{Durability, EventKind, LogLevel, RunEvent, EventCorrelation};

fuzz_target!(|data: &[u8]| {
    // Attempt 1: deserialize raw bytes as EventKind.
    let _ = serde_json::from_slice::<EventKind>(data);

    // Attempt 2: interpret as UTF-8 string and try JSON parsing.
    if let Ok(s) = std::str::from_utf8(data) {
        let _ = serde_json::from_str::<EventKind>(s);

        // Try wrapping in an object to test tagged-enum parsing.
        // EventKind uses #[serde(rename_all = "snake_case")] so we try
        // various envelope shapes.
        let wrapped = format!(r#"{{"kind":{}}}"#, s);
        let _ = serde_json::from_str::<serde_json::Value>(&wrapped);
    }

    // Attempt 3: deserialize as related event types.
    let _ = serde_json::from_slice::<Durability>(data);
    let _ = serde_json::from_slice::<LogLevel>(data);
    let _ = serde_json::from_slice::<RunEvent>(data);
    let _ = serde_json::from_slice::<EventCorrelation>(data);

    // Attempt 4: if we successfully deserialize an EventKind, verify
    // that serializing it back produces valid JSON.
    if let Ok(kind) = serde_json::from_slice::<EventKind>(data) {
        let json = serde_json::to_string(&kind);
        assert!(json.is_ok(), "serialization of valid EventKind must not fail");

        // Round-trip: deserialize the serialized form.
        if let Ok(json_str) = json {
            let roundtrip = serde_json::from_str::<EventKind>(&json_str);
            assert!(
                roundtrip.is_ok(),
                "round-trip deserialization must succeed"
            );
        }
    }

    // Attempt 5: if we get a valid RunEvent, check that its fields are
    // accessible without panic.
    if let Ok(event) = serde_json::from_slice::<RunEvent>(data) {
        let _ = event.id;
        let _ = event.run_id;
        let _ = event.sequence;
        let _ = &event.kind;
        let _ = &event.durability;
        let _ = &event.correlation;
        let _ = &event.causation_id;
        let _ = event.timestamp;
    }
});
