use polkagent_telemetry::{init_telemetry, LogFormat, TelemetryConfig};

#[test]
fn repeated_initialization_returns_error_instead_of_panicking() {
    let config = TelemetryConfig {
        log_level: "info".to_owned(),
        log_format: LogFormat::Pretty,
        otlp_endpoint: None,
        service_name: "polkagent-telemetry-reinitialization-test".to_owned(),
    };

    let _first_guard = init_telemetry(config.clone()).expect("first initialization should succeed");

    let second = std::panic::catch_unwind(|| init_telemetry(config));
    assert!(second.is_ok(), "second initialization must not panic");
    assert!(
        second.expect("panic was checked above").is_err(),
        "second initialization should report the existing global subscriber"
    );
}
