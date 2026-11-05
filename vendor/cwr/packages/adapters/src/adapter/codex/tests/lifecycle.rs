#[test]
fn codex_route_completes_with_only_codex_on_path() {
    const FIXTURE_ENV: &str = "CWR_TEST_CODEX_ONLY_PATH";
    let Some(directory) = std::env::var_os(FIXTURE_ENV) else {
        let script = FAKE_CODEX.replace("#!/usr/bin/env sh", "#!/bin/sh");
        let (directory, _) = fixture(&script);
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "codex_route_completes_with_only_codex_on_path",
                "--nocapture",
            ])
            .env(FIXTURE_ENV, directory.path())
            .env("PATH", directory.path())
            .env("HOME", directory.path().join("home"))
            .env("USER", "fixture-user")
            .env(
                "CWR_CODEX_ADAPTER_EXECUTABLE",
                env!("CARGO_BIN_EXE_cwr-adapter-codex"),
            )
            .output()
            .expect("run route test with an isolated PATH");
        assert!(
            output.status.success(),
            "stdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    };
    let directory = PathBuf::from(directory);
    assert_eq!(std::env::var_os("PATH").unwrap(), directory.as_os_str());
    assert!(!directory.join("deno").exists());
    let mut requested = invocation(&directory.join("codex"));
    requested.host_operations.push(send_message_operation());
    let route = cwr::adapter::ResolvedProcessAdapterRoute::new(
        CODEX_ADAPTER_REF,
        &directory,
        &directory,
        requested.adapter_params.clone(),
        json!({}),
        json!({}),
    );
    let outcome = route
        .run(
            requested,
            &ProcessAdapterStopControl::new(),
            |_| {},
            |_| Ok(json!({"messages": [{"sender": "@helper", "text": "reply"}]})),
        )
        .expect("resolve and run Codex without system Deno");
    let ProcessAdapterOutcome::Completed(terminal) = outcome else {
        panic!("Codex action stopped unexpectedly");
    };
    assert_eq!(terminal.status, 200);
    assert_eq!(terminal.result, Some(json!({"message": "complete"})));
}

#[test]
fn codex_ref_uses_the_calendar_versioned_adapter_collection() {
    assert_eq!(
        CODEX_ADAPTER_REF,
        "cwp://org.coworkerprotocol/2026-09/adapters/codex"
    );
    CwrRef::parse(CODEX_ADAPTER_REF).expect("Codex ref must be canonical");
}

#[test]
fn codex_adapter_retains_provider_stderr_when_startup_fails() {
    let script = "#!/bin/sh\nprintf '%s\\n' 'node runtime unavailable' >&2\nexit 42\n";
    let (directory, fake_codex) = fixture(script);

    let error = run_process_adapter(
        &codex_adapter(directory.path()),
        invocation(&fake_codex),
        &ProcessAdapterStopControl::new(),
        |_| {},
    )
    .expect_err("Codex startup failure must fail the adapter");
    let failure = error
        .downcast_ref::<ProcessAdapterFailure>()
        .expect("process adapter diagnostics");

    assert!(failure.stderr_tail.contains("node runtime unavailable"));
}

#[test]
fn respond_to_message_streams_native_text_and_wraps_the_completed_result() {
    let (directory, fake_codex) = fixture(FAKE_NATIVE_TEXT_CODEX);
    let adapter = codex_adapter(directory.path());
    let requested = native_text_invocation(&fake_codex);
    let emitted = directory.path().join("native-text-delta-emitted");
    let release = directory.path().join("native-text-delta-release");
    let mailbox = AdapterModelOutputMailbox::default();
    let runner_mailbox = mailbox.clone();
    let stop = ProcessAdapterStopControl::new();
    let runner_stop = stop.clone();
    let (outcome_sender, outcome_receiver) = mpsc::channel();
    let runner = thread::spawn(move || {
        let outcome = run_process_adapter_with_model_output_activity_and_controlled_host_calls(
            &adapter,
            requested,
            &runner_stop,
            &runner_mailbox,
            |_| {},
            |_| Ok(()),
            |_| Err(std::io::Error::other("host calls are unavailable").into()),
        );
        outcome_sender.send(outcome).unwrap();
    });

    assert!(wait_for_fixture_marker(&emitted));
    let deadline = Instant::now() + Duration::from_secs(1);
    let streamed = loop {
        let snapshot = mailbox.snapshot();
        if snapshot.output.is_some() || Instant::now() >= deadline {
            break snapshot;
        }
        thread::sleep(Duration::from_millis(10));
    };
    let live = streamed.output.expect("native final-answer delta");
    assert_eq!(
        live.phase,
        cwr::open_responses::v2026_04_24::AssistantPhase::FinalAnswer
    );
    assert_eq!(live.text, "Streaming");
    fs::write(&release, b"release").unwrap();

    let outcome = outcome_receiver
        .recv_timeout(Duration::from_secs(5))
        .expect("native text adapter outcome")
        .expect("native text adapter success");
    runner.join().expect("native text runner");
    let ProcessAdapterOutcome::Completed(terminal) = outcome else {
        panic!("native text action stopped unexpectedly");
    };
    assert_eq!(terminal.status, 200);
    assert_eq!(terminal.summary, "Returned a response.");
    assert_eq!(terminal.result, Some(json!({"text": "Streaming result"})));
    assert!(mailbox.snapshot().output.is_none());
}

#[test]
fn nested_turn_completion_does_not_end_the_parent_action() {
    let (directory, fake_codex) = fixture(FAKE_NESTED_TURN_COMPLETION_CODEX);
    let mut activity = Vec::new();

    let outcome = run_process_adapter_with_activity_and_host_calls(
        &codex_adapter(directory.path()),
        native_text_invocation(&fake_codex),
        &ProcessAdapterStopControl::new(),
        |_| {},
        |event| {
            activity.push(event);
            Ok(())
        },
        |_| Err(std::io::Error::other("host calls are unavailable").into()),
    )
    .expect("nested completion must not terminate the parent action");

    let ProcessAdapterOutcome::Completed(terminal) = outcome else {
        panic!("parent action stopped unexpectedly");
    };
    assert_eq!(terminal.result, Some(json!({"text": "Parent finished."})));
    assert!(activity.iter().any(|activity| matches!(
        &activity.event,
        AdapterActivityEvent::ProviderEvent { method, data, .. }
            if method == "turn/completed"
                && data["turn"]["id"] == "child-turn"
                && data["scope"] == "nested_or_unscoped"
    )));
    assert!(activity.iter().any(|activity| matches!(
        &activity.event,
        AdapterActivityEvent::ProviderEvent { method, data, .. }
            if method == "turn/completed"
                && data["turn"]["id"] == "parent-turn"
                && data["scope"] == "active"
    )));
}
#[test]
fn codex_adapter_translates_progress_host_calls_and_terminal_results() {
    let (directory, fake_codex) = fixture(FAKE_CODEX);
    let mut requested = invocation(&fake_codex);
    requested.host_operations.push(send_message_operation());
    let stop = ProcessAdapterStopControl::new();
    let mut progress = Vec::new();
    let mut activity = Vec::new();
    let mut host_calls = Vec::new();
    let outcome = run_process_adapter_with_activity_and_host_calls(
        &codex_adapter(directory.path()),
        requested,
        &stop,
        |event| progress.push(event),
        |event| {
            activity.push(event);
            Ok(())
        },
        |call| {
            host_calls.push(call);
            Ok(json!({"messages": [{"sender": "@helper", "text": "reply"}]}))
        },
    )
    .expect("run standalone Codex adapter");
    // Restoration is metadata-only activity; explicit numeric progress is preserved.
    assert_eq!(progress.len(), 1);
    assert_eq!(progress[0].sequence, 1);
    assert_eq!(progress[0].status, 102);
    assert_eq!(progress[0].summary, "Working.");
    let restoration = activity.iter().filter_map(|item| match &item.event {
        AdapterActivityEvent::ExecutionActivity { activity } if activity.phase == cwr::adapter::ExecutionPhase::RestoringContext => Some(activity),
        _ => None,
    }).collect::<Vec<_>>();
    assert_eq!(restoration.len(), 2);
    assert_eq!(restoration[0].state, cwr::adapter::ExecutionPhaseState::Started);
    assert_eq!(restoration[1].state, cwr::adapter::ExecutionPhaseState::Completed);
    assert_eq!(restoration[1].completed_parts, restoration[1].total_parts);
    assert!(activity.iter().any(|activity| matches!(
        &activity.event,
        AdapterActivityEvent::ProviderEvent { method, data, .. }
            if method == "item/started"
                && data["item"]["progress_summary"] == "Checking source-control changes."
    )));
    assert!(activity.iter().any(|activity| matches!(
        activity.event,
        AdapterActivityEvent::ProviderThread {
            ref provider,
            ref thread_id,
        } if provider == "codex" && thread_id == "fake-thread"
    )));
    assert!(activity.iter().any(|activity| matches!(
        &activity.event,
        AdapterActivityEvent::ProviderEvent { method, data, .. }
            if method == "item/completed" && data["item"]["text"] == "Visible draft."
    )));
    let commentary = activity
        .iter()
        .filter_map(|activity| match &activity.event {
            AdapterActivityEvent::CompletedCommentary { commentary } => Some(commentary),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(commentary.len(), 2);
    assert_eq!(commentary[0].provider, "codex");
    assert_eq!(commentary[0].thread_id.as_deref(), Some("fake-thread"));
    assert_eq!(commentary[0].response_or_turn_id, "turn-1");
    assert_eq!(commentary[0].item_id, "message-1");
    assert_eq!(commentary[0].text, "Visible draft.");
    assert_eq!(commentary[1].item_id, "message-2");
    assert_eq!(commentary[1].text, "Working.");
    assert_eq!(
        activity
            .iter()
            .map(|activity| activity.sequence)
            .collect::<Vec<_>>(),
        (0..activity.len() as u64).collect::<Vec<_>>()
    );
    assert_eq!(host_calls.len(), 1);
    assert_eq!(host_calls[0].call_id, "send-message-1");
    assert_eq!(host_calls[0].operation, "cwr.send_message");
    assert_eq!(host_calls[0].input["to"], "@helper");
    let ProcessAdapterOutcome::Completed(terminal) = outcome else {
        panic!("Codex adapter stopped unexpectedly");
    };
    assert_eq!(terminal.sequence, 2);
    assert_eq!(terminal.status, 200);
    assert_eq!(terminal.result, Some(json!({"message": "complete"})));
    assert_eq!(
        terminal.continuation.as_ref().unwrap()["thread_id"],
        "fake-thread"
    );
}

#[test]
fn codex_adapter_preserves_exact_progress_and_completed_statuses() {
    let (directory, fake_codex) = fixture(FAKE_NUMERIC_STATUS_CODEX);
    let mut progress = Vec::new();

    let outcome = run_process_adapter(
        &codex_adapter(directory.path()),
        invocation(&fake_codex),
        &ProcessAdapterStopControl::new(),
        |event| progress.push(event),
    )
    .expect("run Codex adapter with numeric status ranges");

    assert_eq!(progress.len(), 1);
    assert_eq!(progress[0].status, 150);
    assert_eq!(progress[0].summary, "Halfway.");
    let ProcessAdapterOutcome::Completed(terminal) = outcome else {
        panic!("Codex adapter stopped unexpectedly");
    };
    assert_eq!(terminal.status, 202);
    assert_eq!(terminal.result, Some(json!({"message": "complete at 202"})));
}

#[test]
fn codex_adapter_rejects_model_status_499_and_accepts_a_corrected_result() {
    let (directory, fake_codex) = fixture(FAKE_HOST_ONLY_STOP_STATUS_CODEX);
    let mut progress = Vec::new();

    let outcome = run_process_adapter(
        &codex_adapter(directory.path()),
        invocation(&fake_codex),
        &ProcessAdapterStopControl::new(),
        |event| progress.push(event),
    )
    .expect("let the model correct a host-only stop status");

    assert!(progress.is_empty());
    let ProcessAdapterOutcome::Completed(terminal) = outcome else {
        panic!("Codex adapter stopped unexpectedly");
    };
    assert_eq!(terminal.status, 204);
    assert_eq!(
        terminal.result,
        Some(json!({"message": "model could not stop"}))
    );
}

#[test]
fn codex_adapter_preserves_a_blocked_status_and_error_detail() {
    let (directory, fake_codex) = fixture(FAKE_BLOCKED_STATUS_CODEX);

    let outcome = run_process_adapter(
        &codex_adapter(directory.path()),
        invocation(&fake_codex),
        &ProcessAdapterStopControl::new(),
        |_| {},
    )
    .expect("run Codex adapter with a blocked status");

    let ProcessAdapterOutcome::Completed(terminal) = outcome else {
        panic!("Codex adapter stopped unexpectedly");
    };
    assert_eq!(terminal.status, 451);
    let error = terminal.error.expect("blocked result error");
    assert_eq!(error.code, "unavailable");
    assert_eq!(error.message.as_deref(), Some("The source is restricted."));
    assert_eq!(error.details, Some(json!({"region": "US"})));
}

#[test]
fn codex_adapter_rejects_opaque_native_threads_without_source_coverage() {
    let (directory, fake_codex) = fixture(FAKE_RESUME_CODEX);
    let mut requested = invocation(&fake_codex);
    requested.continuation = Some(json!("prior-thread"));
    let error = run_process_adapter(
        &codex_adapter(directory.path()),
        requested,
        &ProcessAdapterStopControl::new(),
        |_| {},
    )
    .expect_err("unsafe legacy continuation rejected");
    assert!(error
        .downcast_ref::<ProcessAdapterFailure>()
        .unwrap()
        .stderr_tail
        .contains("confirmed source coverage"));
}

#[test]
fn codex_adapter_emits_visible_activity_before_an_unfinished_turn_fails() {
    let (directory, fake_codex) = fixture(FAKE_UNFINISHED_CODEX);
    let mut activity = Vec::new();
    let error = run_process_adapter_with_activity_and_host_calls(
        &codex_adapter(directory.path()),
        invocation(&fake_codex),
        &ProcessAdapterStopControl::new(),
        |_| {},
        |event| {
            activity.push(event);
            Ok(())
        },
        |_| Err(std::io::Error::other("host calls are unavailable").into()),
    )
    .expect_err("turn without cwr.return_result must fail");
    assert!(error.to_string().contains("before returning a result"));
    assert!(activity.iter().any(|activity| matches!(
        &activity.event,
        AdapterActivityEvent::ProviderThread { thread_id, .. }
            if thread_id == "unfinished-thread"
    )));
    assert!(activity.iter().any(|activity| matches!(
        &activity.event,
        AdapterActivityEvent::ProviderEvent { method, data, .. }
            if method == "item/completed"
                && data["item"]["text"] == "I was still working."
    )));
    assert!(activity.iter().any(|activity| matches!(
        &activity.event,
        AdapterActivityEvent::CompletedCommentary { commentary }
            if commentary.response_or_turn_id == "turn-unfinished"
                && commentary.item_id == "message-unfinished"
                && commentary.text == "I was still working."
    )));
    assert!(activity.iter().any(|activity| matches!(
        &activity.event,
        AdapterActivityEvent::ProviderEvent { method, .. } if method == "turn/completed"
    )));
}

#[test]
fn codex_adapter_does_not_retain_a_terminal_result_until_the_turn_succeeds() {
    let script = r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*)
      printf '%s\n' '{"id":"cwr-initialize","result":{}}' ;;
    *'"method":"thread/start"'*)
      printf '%s\n' '{"id":"cwr-thread-start","result":{"thread":{"id":"terminal-thread"}}}' ;;
    *'"method":"thread/inject_items"'*)
      printf '%s\n' '{"id":"cwr-observation-inject","result":{}}' ;;
    *'"method":"turn/start"'*)
      printf '%s\n' '{"id":"cwr-turn-start","result":{"turn":{"id":"terminal-turn"}}}'
      printf '%s\n' '{"id":2,"method":"item/tool/call","params":{"namespace":"cwp","tool":"cwr--return--result","arguments":{"status":200,"summary":"Done.","result":{"message":"not committed"}}}}' ;;
    *'"id":2'*'"result"'*)
      printf '%s\n' '{"id":3,"method":"item/tool/call","params":{"callId":"late-send","namespace":"cwp","tool":"cwr--send--message","arguments":{"to":"@helper","text":"too late"}}}' ;;
    *'"id":3'*'"success":false'*)
      printf '%s\n' '{"method":"turn/completed","params":{"threadId":"terminal-thread","turn":{"id":"terminal-turn","status":"failed","error":{"message":"response was not committed"}}}}' ;;
    *'"id":3'*) exit 72 ;;
  esac
done
"#;
    let (directory, fake_codex) = fixture(script);
    let mut activity = Vec::new();
    let mut host_calls = 0;
    let mut requested = invocation(&fake_codex);
    requested.host_operations.push(send_message_operation());
    let error = run_process_adapter_with_activity_and_host_calls(
        &codex_adapter(directory.path()),
        requested,
        &ProcessAdapterStopControl::new(),
        |_| {},
        |event| {
            activity.push(event);
            Ok(())
        },
        |_| {
            host_calls += 1;
            Ok(json!({"messages": []}))
        },
    )
    .expect("failed terminal turn returns a structured result");
    let ProcessAdapterOutcome::Completed(failure) = error else { panic!("terminal failure") };
    assert_eq!(failure.status, 500);
    assert!(failure.summary.contains("response was not committed"));
    assert!(failure.result.is_none());
    assert!(failure.continuation.is_none());
    assert_eq!(failure.error.unwrap().code, "codex_execution_failed");
    assert!(!activity.iter().any(|activity| matches!(
        activity.event,
        AdapterActivityEvent::ProviderEvent { ref method, .. }
            if method == "cwr.working_state"
    )));
    assert_eq!(host_calls, 0, "host calls stop after a terminal result");
}
