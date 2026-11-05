use super::*;
use std::{
    fs,
    os::unix::fs::{symlink, PermissionsExt},
};

#[test]
fn browse_overage_persists_only_attribution_and_byte_counts() {
    let temporary = tempfile::tempdir().unwrap();
    let recorder = DiagnosticsRecorder::new(&temporary.path().join("state.sqlite"));
    recorder.record_browse_overage(
        &crate::v2026_09::conversation::BrowseMessageOverage {
            conversation_ref: "cwp://example.test/conversations/one".into(),
            message_ref: "cwp://example.test/messages/two".into(),
            operation: "conversation_browse_page",
            payload_bytes: 8_500_000,
            limit_bytes: 8 * 1024 * 1024,
        },
        Some("cwp://example.test/invocations/three"),
        Some("Authorization: Bearer request-private-value"),
        Some("operation-private-value"),
    );
    let directory = recorder.directory.clone();
    drop(recorder);
    let saved =
        store::records_for_invocation(&directory, "cwp://example.test/invocations/three").unwrap();
    let record = &saved.records[0].value;
    assert_eq!(record["source"], "conversation_browse_overage");
    assert_eq!(
        record["conversation_ref"],
        "cwp://example.test/conversations/one"
    );
    assert_eq!(record["message_ref"], "cwp://example.test/messages/two");
    assert_eq!(record["operation"], "conversation_browse_page");
    assert_eq!(record["payload_bytes"], 8_500_000);
    assert_eq!(record["limit_bytes"], 8 * 1024 * 1024);
    assert_eq!(
        record["request_id_sha256"],
        hex::encode(Sha256::digest(
            b"Authorization: Bearer request-private-value"
        ))
    );
    assert_eq!(
        record["operation_id_sha256"],
        hex::encode(Sha256::digest(b"operation-private-value"))
    );
    assert!(record["event_id"].as_str().is_some_and(|id| !id.is_empty()));
    assert!(record.get("text").is_none());
    assert!(record.get("action").is_none());
    assert!(record.get("payload_json").is_none());
    assert!(!record.to_string().contains("private-value"));
}

#[test]
fn failure_survives_recorder_restart_with_attempt_and_stderr() {
    let temp = tempfile::tempdir().unwrap();
    let recorder = DiagnosticsRecorder::new(&temp.path().join("state.json"));
    let context = MetricContext {
        runtime_ref: "runtime".into(),
        invocation_ref: "invocation".into(),
        conversation_ref: None,
        parent_invocation_ref: None,
        attempt: 2,
        model_run_ref: None,
        actor_ref: "actor".into(),
        model: None,
        reasoning_effort: None,
        invocation_continuation_supplied: false,
        private_history_supplied: false,
        context_bytes: 0,
    };
    recorder.record_failure(
        &context,
        &ProcessAdapterFailure {
            message: "exited".into(),
            exit_code: Some(23),
            signal: None,
            stdout_state: "eof_before_result".into(),
            stderr_tail: "diagnostic marker".into(),
            stderr_total_bytes: 17,
            stderr_truncated: false,
            stderr_complete: false,
            stderr_capture_error: None,
        },
    );
    let directory = recorder.directory.clone();
    drop(recorder);
    let restarted = DiagnosticsRecorder::new(&temp.path().join("state.json"));
    let file = directory.join("wlpdb.sqlite3");
    let saved = store::records_for_invocation(&directory, "invocation").unwrap();
    let record = &saved.records[0].value;
    assert_eq!(record["invocation_ref"], "invocation");
    assert_eq!(record["attempt"], 2);
    assert_eq!(record["process"]["stderr_tail"], "diagnostic marker");
    assert_eq!(record["process"]["stderr_complete"], false);
    assert_eq!(
        fs::metadata(file).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(directory).unwrap().permissions().mode() & 0o777,
        0o700
    );
    let evidence = restarted.evidence_for_invocation("invocation");
    assert_eq!(evidence.availability, "available");
    assert_eq!(evidence.records.len(), 1);
    assert_eq!(evidence.records[0].exit_code, Some(23));
    assert_eq!(
        evidence.records[0].stdout_state.as_deref(),
        Some("eof_before_result")
    );
    assert_eq!(
        evidence.records[0].stderr_tail.as_deref(),
        Some("diagnostic marker")
    );
    assert!(evidence.records[0]
        .raw_log_locator
        .contains("wlpdb.sqlite3#cwp://"));
    drop(restarted);
}

#[test]
fn evidence_lookup_distinguishes_missing_and_unreadable_stores() {
    let temp = tempfile::tempdir().unwrap();
    let recorder = DiagnosticsRecorder::new(&temp.path().join("state.json"));
    assert_eq!(
        recorder.evidence_for_invocation("missing").availability,
        "not_recorded"
    );
    drop(recorder);

    let redirected = tempfile::tempdir().unwrap();
    symlink(temp.path(), redirected.path().join("diagnostics")).unwrap();
    let recorder = DiagnosticsRecorder::new(&redirected.path().join("state.json"));
    assert_eq!(
        recorder.evidence_for_invocation("missing").availability,
        "unreadable"
    );
}

#[test]
fn sqlite_retention_preserves_historic_files_and_rejects_symlinks() {
    let temp = tempfile::tempdir().unwrap();
    let directory = temp.path().join("diagnostics");
    fs::create_dir(&directory).unwrap();
    let historic = directory.join("failures-0000000090.jsonl");
    fs::write(&historic, "historic").unwrap();
    store::write_record(
        &directory,
        &json!({"recorded_at_unix_ms": 90 * 86_400_000_u64, "invocation_ref": "old"}),
    )
    .unwrap();
    let path = store::write_record(
        &directory,
        &json!({"recorded_at_unix_ms": 100 * 86_400_000_u64, "invocation_ref": "new"}),
    )
    .unwrap();
    assert!(store::records_for_invocation(&directory, "old")
        .unwrap()
        .records
        .is_empty());
    assert_eq!(
        store::records_for_invocation(&directory, "new")
            .unwrap()
            .records
            .len(),
        1
    );
    assert_eq!(fs::read_to_string(&historic).unwrap(), "historic");
    assert_eq!(
        fs::read_dir(&directory)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.path().extension().is_some_and(|s| s == "jsonl"))
            .count(),
        1
    );
    fs::remove_file(&path).unwrap();
    let outside = temp.path().join("outside");
    fs::write(&outside, "untouched").unwrap();
    symlink(&outside, &path).unwrap();
    assert!(store::write_record(&directory, &json!({})).is_err());
    assert_eq!(fs::read_to_string(&outside).unwrap(), "untouched");
    let redirected = temp.path().join("redirected");
    symlink(&directory, &redirected).unwrap();
    assert!(store::write_record(&redirected, &json!({})).is_err());
}
#[test]
fn historic_jsonl_evidence_remains_readable() {
    let temp = tempfile::tempdir().unwrap();
    let directory = temp.path().join("diagnostics");
    fs::create_dir(&directory).unwrap();
    fs::write(
        directory.join("failures-0000000090.jsonl"),
        "{\"invocation_ref\":\"legacy\",\"error\":\"old failure\"}\n",
    )
    .unwrap();
    let recorder = DiagnosticsRecorder::new(&temp.path().join("state.json"));
    let evidence = recorder.evidence_for_invocation("legacy");
    assert_eq!(evidence.availability, "available");
    assert!(evidence.records[0].raw_log_locator.contains(".jsonl:1"));
}

#[test]
fn diagnostic_text_is_capped_and_known_credentials_are_removed() {
    let text = text::scrub(
        "useful error\nAuthorization: Bearer secret\napi_key=secret\nsk-secret\n",
        4096,
    );
    assert!(text.contains("useful error"));
    assert!(!text.contains("secret"));
    assert!(text::scrub(&"é".repeat(2000), 101).len() <= 101);
}

#[test]
fn unavailable_store_does_not_fail_execution() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("diagnostics"), "occupied").unwrap();
    let recorder = DiagnosticsRecorder::new(&temp.path().join("state"));
    recorder
        .writer
        .sender
        .as_ref()
        .unwrap()
        .try_send(json!({}))
        .unwrap();
    drop(recorder);
    assert_eq!(
        fs::read_to_string(temp.path().join("diagnostics")).unwrap(),
        "occupied"
    );
}

#[test]
fn oversized_credentials_are_redacted_before_retaining_the_tail() {
    for prefix in ["Authorization: Bearer ", "sk-", "github_pat_"] {
        let secret = "private".repeat(4096);
        let input = format!("{prefix}{secret}\nnormal failure detail");
        let saved = text::scrub(&input, 16 * 1024);
        assert!(!saved.contains("private"));
        assert!(saved.contains("redacted"));
        assert!(saved.ends_with("normal failure detail"));
        assert!(saved.len() <= 16 * 1024);
    }
}
