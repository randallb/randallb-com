#[test]
fn failed_startup_records_attempt_end_and_storage_failure_does_not_fail_execution() {
    for block_metrics in [false, true] {
        let temporary = tempfile::tempdir().unwrap();
        let state_path = temporary.path().join("conversation-admission.json");
        if block_metrics {
            fs::create_dir(temporary.path().join(crate::metrics::LOCAL_METRICS_DATABASE_FILENAME)).unwrap();
        }
        let runtime = LocalRuntime::open_with_state_path(
            fixture_workspace(),
            &state_path,
            fixture_host_identity(),
        )
        .unwrap();
        let admission = runtime.admit_text(None, "@helper metric fixture").unwrap();
        runtime.lock().unwrap().graph.actor_integrations.clear();
        let turn = runtime
            .execute_admission(&admission, |event| {
                if matches!(event, LocalRuntimeEvent::InvocationStarted { .. }) {
                    std::thread::sleep(std::time::Duration::from_millis(30));
                }
            })
            .unwrap();
        assert!(turn.invocation.unwrap().state.is_terminal());
        runtime.metrics.flush();
        if !block_metrics {
            let records = read_metric_records(temporary.path());
            let content = serde_json::to_string(&records).unwrap();
            assert_eq!(records.len(), 4);
            assert_eq!(records[0]["checkpoint"], "attempt_started");
            assert_eq!(records[3]["checkpoint"], "attempt_ended");
            assert_eq!(records[3]["measurements"]["outcome"], "failed");
            assert!(records[3]["measurements"]["total_turn_ms"].is_u64());
            assert_eq!(
                records[1]["measurements"]["stage"],
                "invocation_started_observer"
            );
            assert!(records[1]["measurements"]["duration_ms"].as_u64().unwrap() >= 30);
            assert_eq!(records[2]["measurements"]["stage"], "route_selection");
            assert!(records[0]["measurements"]["metrics_metadata_ms"].is_u64());
            assert!(!content.contains("metric fixture"));
            assert!(!content.contains("first_text"));
        }
    }
}

#[test]
#[ignore = "manual read-scaling characterization"]
fn characterize_host_conversation_projection() {
    for conversation_count in [10, 50, 100] {
        let temporary = tempfile::tempdir().unwrap();
        let runtime = LocalRuntime::open_with_state_path(
            fixture_workspace(),
            temporary.path().join("conversation-admission.json"),
            fixture_host_identity(),
        )
        .unwrap();
        for conversation_index in 0..conversation_count {
            let first = runtime
                .admit_text(
                    None,
                    format!("@helper conversation {conversation_index}: {}", "x".repeat(180)),
                )
                .unwrap();
            let conversation_ref = first.conversation_ref().to_owned();
            for message_index in 1..10 {
                runtime
                    .admit_text(
                        Some(&conversation_ref),
                        format!("message {message_index}: {}", "x".repeat(180)),
                    )
                    .unwrap();
            }
        }
        let iterations = 10;
        let started = std::time::Instant::now();
        let mut encoded_bytes = 0;
        for _ in 0..iterations {
            let snapshot = runtime.host_conversations().unwrap();
            encoded_bytes = serde_json::to_vec(&snapshot).unwrap().len();
        }
        println!(
            "host_conversation_projection conversations={conversation_count} messages={} encoded_bytes={encoded_bytes} average_us={}",
            conversation_count * 10,
            started.elapsed().as_micros() / iterations,
        );
    }
}

#[test]
fn slow_queued_invocation_is_logged_before_stop_without_running_a_provider() {
    let temporary = tempfile::tempdir().unwrap();
    let state_path = temporary.path().join("conversation-admission.json");
    let mut runtime = LocalRuntime::open_with_state_path(
        fixture_workspace(), &state_path, fixture_host_identity(),
    ).unwrap();
    runtime.metrics = crate::metrics::MetricsRecorder::with_slow_threshold(
        &state_path, std::time::Duration::from_millis(20),
    );
    runtime.lock().unwrap().admission.metrics = Some(runtime.metrics.clone());
    let admission = runtime.admit_text(None, "@helper secret message must never appear in metrics").unwrap();
    // The writer creates the database and its schema lazily with the first
    // slow-invocation record. A read can observe the new SQLite file before
    // that schema transaction commits, so treat the store as not ready while
    // polling. Reads after `flush` remain strict below.
    let read_records = || try_read_metric_records(temporary.path()).unwrap_or_default();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        if read_records().iter().any(|row| row["checkpoint"] == "slow_invocation_threshold") { break; }
        assert!(std::time::Instant::now() < deadline, "timer did not log queued work");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let invocation_ref = admission.record.invocation_ref.as_deref().unwrap();
    assert_eq!(runtime.lock().unwrap().admission.invocation_record(invocation_ref).unwrap().state,
        ActionInvocationState::Admitted);
    runtime.stop_invocation(invocation_ref, Some("private stop reason".into())).unwrap();
    runtime.metrics.flush();
    let records = read_records();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0]["checkpoint"], "slow_invocation_threshold");
    assert_eq!(records[0]["measurements"]["state"], "admitted");
    assert_eq!(records[1]["checkpoint"], "slow_invocation_ended");
    assert_eq!(records[1]["measurements"]["outcome"], "stopped");
    for record in &records {
        assert_eq!(record["invocation_ref"], invocation_ref);
        assert_eq!(record["measurements"]["source_message_ref"], admission.record.source_message_ref);
        assert_eq!(record["measurements"]["conversation_ref"], admission.record.conversation_ref);
    }
    let encoded = serde_json::to_string(&records).unwrap();
    assert!(!encoded.contains("secret message"));
    assert!(!encoded.contains("private stop reason"));
}

fn read_metric_records(root: &std::path::Path) -> Vec<Value> {
    try_read_metric_records(root).expect("metrics store is readable after flush")
}

fn try_read_metric_records(root: &std::path::Path) -> Option<Vec<Value>> {
    let mut db = crate::cwpdb::NativeStore::open_read_only(
        root.join(crate::metrics::LOCAL_METRICS_DATABASE_FILENAME),
    )
    .ok()?;
    let page = db.metric_records(None, 1000).ok()?;
    assert!(!page.truncated, "metrics fixture exceeds its read limit");
    Some(page.records)
}
