//! Proves telemetry actually reaches a collector, rather than trusting that
//! wiring an allow-listed target and a `report_error` call means what it looks
//! like it means.
//!
//! Runs against a local fake OTLP receiver (`mockito`, already a dev-dependency)
//! standing in for the household's real collector — the exported bytes travel
//! the same OTLP/HTTP path either way. `telemetry::init` is process-global and
//! callable once, which is exactly why this lives in its own `tests/*.rs`
//! binary: the `thoth_lib` unit-test binary never calls `init`, so there is no
//! other caller to race.

#[test]
fn a_reported_error_and_an_allow_listed_span_both_reach_the_collector() {
    let mut server = mockito::Server::new();
    let logs_mock = server
        .mock("POST", "/v1/logs")
        .with_status(200)
        .expect_at_least(1)
        .create();
    let traces_mock = server
        .mock("POST", "/v1/traces")
        .with_status(200)
        .expect_at_least(1)
        .create();

    // SAFETY: this test is the only caller of `telemetry::init` in the whole
    // binary (see the module doc), and nothing else in this process reads or
    // writes these two variables.
    unsafe {
        std::env::set_var("OTEL_EXPORTER_OTLP_ENDPOINT", server.url());
        std::env::remove_var("OTEL_EXPORTER_OTLP_HEADERS_HELPER");
    }

    let guard = telemetry::init("thoth-test", "0.0.0", &["thoth_telemetry_export_test"]);
    assert!(
        !guard.exporter_is_from_env() || guard.exporter().is_some(),
        "the endpoint just set should have built a real exporter"
    );

    // A caught error, on the crate's own force-allowed target — this is what
    // the app calls at every failure site a user would feel.
    telemetry::report_error("integration_test_error");

    // An allow-listed span, standing in for the per-command telemetry span
    // every Tauri IPC command now carries.
    tracing::info_span!(target: "thoth_telemetry_export_test", "integration_test_span").in_scope(
        || {
            tracing::info!(
                target: "thoth_telemetry_export_test",
                "integration test event"
            );
        },
    );

    // Drop flushes both batch processors, bounded, before returning — see
    // telemetry-rs's own `Guard` docs. The exporter's HTTP client is blocking,
    // so the POSTs below have already landed by the time this returns.
    drop(guard);

    logs_mock.assert();
    traces_mock.assert();
}
