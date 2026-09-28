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
//!
//! Since the move to `tauri_plugin_telemetry` (moved from the standalone
//! telemetry-rs repo into the factory's `kits/rust`, then onto the public
//! radar-hooves/app-factory, master-project#233), every Tauri IPC command's
//! span is opened by the plugin's own `tauri_plugin_telemetry::traced` (an
//! async command), `traced_sync` (a command that must stay `fn` and can
//! fail), or `traced_sync_value` (v2026.9.34 — a command that must stay `fn`
//! and cannot fail, so its span's outcome is always `ok` with no `Result`,
//! `Infallible` or `.unwrap()` at the call site) — rather than a hand-rolled
//! `#[tracing::instrument]`. All three land on the plugin's own
//! `COMMAND_SPAN_TARGET`; this test calls each directly, the same way a real
//! command body does, to prove all three reach the collector, not only
//! Thoth's own curated target.

#[test]
fn a_reported_error_a_curated_span_and_all_three_command_span_shapes_reach_the_collector() {
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

    // `tauri_plugin_telemetry::init`'s own allow-list handling adds
    // `COMMAND_SPAN_TARGET` automatically; this test drives `telemetry::init`
    // directly (there is no Tauri `App` here to plug into), so it adds that
    // target itself, exactly as the plugin would.
    let guard = telemetry::init(
        "thoth-test",
        "0.0.0",
        &[
            "thoth_telemetry_export_test",
            tauri_plugin_telemetry::COMMAND_SPAN_TARGET,
        ],
    );
    assert!(
        !guard.exporter_is_from_env() || guard.exporter().is_some(),
        "the endpoint just set should have built a real exporter"
    );

    // A caught error, on the crate's own force-allowed target — this is what
    // the app calls at every failure site a user would feel.
    telemetry::report_error("integration_test_error");

    // A curated domain span, standing in for Thoth's own hand-built spans
    // (dictation, process_audio, transcription, enhancement).
    tracing::info_span!(target: "thoth_telemetry_export_test", "integration_test_span").in_scope(
        || {
            tracing::info!(
                target: "thoth_telemetry_export_test",
                "integration test event"
            );
        },
    );

    // The real per-command span mechanism every Tauri IPC command wrapped in
    // `tauri_plugin_telemetry::traced` now carries — name, duration, outcome.
    // `telemetry::init` above must run on a plain thread, never inside a Tokio
    // task (it builds a blocking `reqwest` client), so the runtime is built
    // here and used only to drive this one `.await` — the same pattern the
    // `telemetry` crate's own tests use for a swap from inside a runtime.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a runtime");
    runtime
        .block_on(tauri_plugin_telemetry::traced(
            "integration_test_command",
            async { Ok::<(), &'static str>(()) },
        ))
        .expect("the traced future's own Ok is untouched");

    // The sync counterpart for a fallible command — Tauri's own setup hook, a
    // tray-menu builder, and many of Thoth's own commands (config::set_config
    // and the rest) call this, never `traced`, because they are also plain
    // Rust functions called directly elsewhere and cannot become `async fn`.
    tauri_plugin_telemetry::traced_sync("integration_test_sync_command", || {
        Ok::<(), &'static str>(())
    })
    .expect("the traced_sync closure's own Ok is untouched");

    // The sync counterpart for a command that cannot fail at all — most of
    // Thoth's own sync commands (config::get_config and the rest): no
    // `Result`, `Infallible` or `.unwrap()` anywhere at the call site, and
    // the span's own outcome is always `ok`.
    let value =
        tauri_plugin_telemetry::traced_sync_value("integration_test_sync_value_command", || 42);
    assert_eq!(
        value, 42,
        "the traced_sync_value closure's own return is untouched"
    );

    // Drop flushes both batch processors, bounded, before returning — see the
    // crate's own `Guard` docs. The exporter's HTTP client is blocking, so the
    // POSTs below have already landed by the time this returns.
    drop(guard);

    logs_mock.assert();
    traces_mock.assert();
}
