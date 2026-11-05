#[test]
fn large_context_preserves_complete_source_and_delegates_compaction_to_codex() {
    let (directory, fake_codex) = fixture(FAKE_NATIVE_CONTEXT_CODEX);
    let mut requested = native_text_invocation(&fake_codex);
    // A legacy setting remains accepted, but Codex owns the actual model limit.
    requested.adapter_params["context_window_tokens"] = json!(4096);
    requested.host_operations.push(send_message_operation());
    let mut messages = vec![
        json!({"message_ref":"early","author":"a","text":"Keep this earlier constraint"}),
    ];
    messages.extend((0..372).map(|index| json!({
        "message_ref":format!("backlog-{index}"),
        "author":"b",
        "text":format!("backlog-{index}:{}", "x".repeat(6_100)),
    })));
    messages.push(json!({"message_ref":"late","author":"a","reply_to":"early",
        "text":"A later correction"}));
    assert_eq!(messages.len(), 374);
    requested.thread["messages"] = json!(messages);
    let exact_input = requested.input.clone();
    let mut activity = Vec::new();
    let mut checkpoints = Vec::new();
    let outcome = run_process_adapter_with_activity_and_host_calls(
        &codex_adapter(directory.path()),
        requested,
        &ProcessAdapterStopControl::new(),
        |_| {},
        |event| {
            if let AdapterActivityEvent::ProviderEvent { method, data, .. } = &event.event {
                if method == "cwr.adapter_checkpoint" {
                    checkpoints.push(data.clone());
                }
            }
            activity.push(event);
            Ok(())
        },
        |_| panic!("history delivery and native compaction must not call host tools"),
    )
    .expect("large context completes through native compaction");
    let ProcessAdapterOutcome::Completed(terminal) = outcome else {
        panic!("native context action stopped unexpectedly");
    };
    assert_eq!(
        terminal.result,
        Some(json!({"text":"Native context complete."}))
    );

    let requests = native_context_requests(directory.path());
    let compactions = requests
        .iter()
        .enumerate()
        .filter(|(_, request)| request["method"] == "thread/compact/start")
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    assert!(!compactions.is_empty());
    let live = requests
        .iter()
        .filter(|request| request["method"] == "turn/start")
        .collect::<Vec<_>>();
    assert_eq!(live.len(), 1);
    let live_request: serde_json::Value =
        serde_json::from_str(live[0]["params"]["input"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(live_request["params"], exact_input);

    // Reassemble the wire payload, including escaped Unicode across transport
    // fragments, to verify that earlier constraints and later replies survive.
    let injected = requests
        .iter()
        .filter(|request| request["method"] == "thread/inject_items")
        .collect::<Vec<_>>();
    let mut source_records: Vec<(String, String)> = Vec::new();
    for request in &injected {
        for item in request["params"]["items"].as_array().unwrap() {
            assert_eq!(item["type"], "message");
            assert_eq!(item["role"], "user");
            for content in item["content"].as_array().unwrap() {
                assert_eq!(content["type"], "input_text");
                let (attribution, fragment) =
                    content["text"].as_str().unwrap().split_once('\n').unwrap();
                let source = attribution
                    .strip_prefix("Historical CWR source record ")
                    .unwrap()
                    .split_once(". Part ")
                    .unwrap()
                    .0;
                assert!(fragment.len() <= 32 * 1024);
                if source_records
                    .last()
                    .is_none_or(|(previous, _)| previous != source)
                {
                    source_records.push((source.to_owned(), String::new()));
                }
                source_records.last_mut().unwrap().1.push_str(fragment);
            }
        }
    }
    let imported_messages = source_records
        .iter()
        .filter(|(source, _)| {
            source
                .strip_prefix("requesting/")
                .and_then(|path| path.chars().next())
                .is_some_and(|first| first.is_ascii_digit())
        })
        .map(|(_, body)| serde_json::from_str::<serde_json::Value>(body).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(imported_messages, messages);
    let last_injection = requests
        .iter()
        .rposition(|request| request["method"] == "thread/inject_items")
        .unwrap();
    assert!(compactions[0] < last_injection);
    let live_start = requests
        .iter()
        .position(|request| request["method"] == "turn/start")
        .unwrap();
    assert!(last_injection < live_start);

    let ready = checkpoints.last().unwrap();
    assert_eq!(ready["version"], 5);
    assert_eq!(ready["live_started"], true);
    assert_eq!(ready["pending"], false);
    assert_eq!(
        ready["confirmed_items"].as_u64().unwrap() as usize,
        injected.len()
    );
    assert!(checkpoints[..checkpoints.len() - 1]
        .iter()
        .all(|state| state["live_started"] == false));
    assert!(ready["compactions"].as_u64().unwrap() >= 1);
    assert!(ready["estimated_input_tokens"].as_u64().is_some());
    assert!(checkpoints
        .iter()
        .all(|state| state.get("compacted_items").is_none()
            && state.get("working_bytes").is_none()));
    assert!(activity.iter().any(|activity| matches!(
        &activity.event,
        AdapterActivityEvent::ProviderEvent { method, data, .. }
            if method == "item/completed" && data["item"]["type"] == "contextCompaction"
    )));
}

#[test]
fn native_context_window_failure_preserves_provider_error_and_never_replays_execution() {
    let (directory, fake_codex) = fixture(FAKE_NATIVE_CONTEXT_CODEX);
    fs::write(directory.path().join("native-context-fail"), b"fail").unwrap();
    let mut requested = native_text_invocation(&fake_codex);
    requested.host_operations.push(send_message_operation());
    let mut checkpoint = None;
    let error = run_process_adapter_with_activity_and_host_calls(
        &codex_adapter(directory.path()),
        requested.clone(),
        &ProcessAdapterStopControl::new(),
        |_| {},
        |activity| {
            if let AdapterActivityEvent::ProviderEvent { method, data, .. } = activity.event {
                if method == "cwr.adapter_checkpoint" {
                    checkpoint = Some(data);
                }
            }
            Ok(())
        },
        |_| panic!("native context failure must not execute host tools"),
    )
    .expect("provider failure returns a structured result");
    let ProcessAdapterOutcome::Completed(failure) = error else { panic!("terminal failure") };
    assert_eq!(failure.status, 500);
    assert!(failure.summary.contains("Model context window exceeded after native compaction"));
    assert_eq!(failure.error.unwrap().code, "codex_execution_failed");
    let checkpoint = checkpoint.expect("retain source coverage before starting native turn");
    assert_eq!(checkpoint["live_started"], true);
    assert_eq!(checkpoint["pending"], false);
    let requests = native_context_requests(directory.path());
    assert_eq!(
        requests
            .iter()
            .filter(|request| request["method"] == "turn/start")
            .count(),
        1
    );
    assert!(requests
        .iter()
        .all(|request| request["method"] != "thread/compact/start"));

    requested.continuation = Some(checkpoint);
    let replay_error = run_process_adapter_with_activity_and_host_calls(
        &codex_adapter(directory.path()),
        requested,
        &ProcessAdapterStopControl::new(),
        |_| {},
        |_| Ok(()),
        |_| panic!("failed native turn must not replay host tools"),
    )
    .expect_err("a submitted native turn must not be replayed automatically");
    assert!(replay_error
        .downcast_ref::<ProcessAdapterFailure>()
        .unwrap()
        .stderr_tail
        .contains("automatic replay is unsupported"));
    assert_eq!(native_context_requests(directory.path()), requests);
}

fn native_context_requests(directory: &Path) -> Vec<serde_json::Value> {
    fs::read_to_string(directory.join("native-context-requests.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}
