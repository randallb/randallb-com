fn tier_fixture() -> (TempDir, PathBuf) {
    let script = FAKE_NATIVE_CONTEXT_CODEX
        .replace("*'\"method\":\"thread/start\"'*)", "*'\"method\":\"thread/start\"'*|*'\"method\":\"thread/resume\"'*|*'\"method\":\"thread/fork\"'*)")
        .replace("    *'\"method\":\"turn/start\"'*)", r#"    *'"method":"model/list"'*)
      if [ -e unsupported-model ]; then
        printf '%s\n' '{"id":"cwr-service-tier-models","result":{"data":[{"model":"fixture-model","serviceTiers":[{"id":"default"}]}],"nextCursor":null}}'
      elif [ -e priority-model ]; then
        printf '%s\n' '{"id":"cwr-service-tier-models","result":{"data":[{"model":"fixture-model","serviceTiers":[{"id":"priority"}]}],"nextCursor":null}}'
      else
        printf '%s\n' '{"id":"cwr-service-tier-models","result":{"data":[{"model":"fixture-model","serviceTiers":[{"id":"default"},{"id":"fast"}]}],"nextCursor":null}}'
      fi
      ;;
    *'"method":"turn/start"'*)"#);
    fixture(&script)
}
#[test]
fn host_tier_preference_applies_to_new_and_resumed_turns_across_workspaces() {
    let (host, _) = tier_fixture();
    let path = host.path().join("tier.json");
    for tier in ["default", "fast", "default", "fast"] {
        fs::write(&path, serde_json::to_vec(&json!({"service_tier":tier})).unwrap()).unwrap();
        let (workspace, fake) = tier_fixture();
        fs::write(workspace.path().join("priority-model"), "").unwrap();
        let adapter = codex_adapter(workspace.path()).with_environment("CWR_CODEX_SERVICE_TIER_FILE", &path);
        let mut request = native_text_invocation(&fake);
        request.adapter_params["model"] = json!("fixture-model");
        request.adapter_params["reasoning_effort"] = json!("high");
        request.conversation_continuity = true;
        request.thread["working_conversation"] = json!({"ref":"cwp://example.test/conversations/working"});
        let mut state = None;
        let outcome = run_process_adapter_with_activity_and_host_calls(&adapter, request.clone(),
            &ProcessAdapterStopControl::new(), |_| {}, |activity| {
                if let AdapterActivityEvent::ProviderEvent { method, data, .. } = activity.event {
                    if method == "cwr.working_state" { state = Some(data); }
                }
                Ok(())
            }, |_| panic!("no tools")).unwrap();
        let ProcessAdapterOutcome::Completed(terminal) = outcome else { panic!("stopped"); };
        assert_eq!(terminal.status, 200);
        let requests = native_context_requests(workspace.path());
        let turn = requests.iter().find(|r| r["method"] == "turn/start").unwrap();
        assert_eq!(turn["params"]["serviceTierForTurn"], tier);
        assert!(turn["params"].get("effort").is_none());
        let thread = requests.iter().find(|r| r["method"] == "thread/start").unwrap();
        assert_eq!(thread["params"]["config"]["model_reasoning_effort"], "high");
        assert_eq!(thread["params"]["config"]["features.fast_mode"], true);
        // Flip after the completed Fast turn, keeping the provider continuation intact.
        fs::write(&path, br#"{"service_tier":"default"}"#).unwrap();
        request.invocation_ref = "cwp://example.test/invocations/next".into();
        request.thread["working_conversation"]["state"] = state.expect("settled state");
        let outcome = run_process_adapter(&adapter, request, &ProcessAdapterStopControl::new(), |_| {}).unwrap();
        let ProcessAdapterOutcome::Completed(terminal) = outcome else { panic!("stopped"); };
        assert_eq!(terminal.status, 200);
        let resumed = native_context_requests(workspace.path());
        assert!(resumed.iter().any(|r| r["method"] == "thread/resume"));
        assert!(!resumed.iter().any(|r| r["method"] == "thread/fork"));
        assert_eq!(resumed.iter().rfind(|r| r["method"] == "turn/start").unwrap()["params"]["serviceTierForTurn"], "default");
    }
}
#[test]
fn unsupported_fast_model_and_missing_preference_use_standard_speed() {
    let (workspace, fake) = tier_fixture();
    let path = workspace.path().join("tier.json");
    let adapter = codex_adapter(workspace.path()).with_environment("CWR_CODEX_SERVICE_TIER_FILE", &path);
    for content in [None, Some("{broken"), Some(r#"{"service_tier":"fast"}"#)] {
        if let Some(content) = content { fs::write(&path, content).unwrap(); }
        fs::write(workspace.path().join("unsupported-model"), "").unwrap();
        let mut request = native_text_invocation(&fake);
        request.adapter_params["model"] = json!("fixture-model");
        let outcome = run_process_adapter(&adapter, request, &ProcessAdapterStopControl::new(), |_| {}).unwrap();
        let ProcessAdapterOutcome::Completed(terminal) = outcome else { panic!("stopped"); };
        assert_eq!(terminal.status, 200);
        let requests = native_context_requests(workspace.path());
        assert_eq!(requests.iter().rfind(|r| r["method"] == "turn/start").unwrap()["params"]["serviceTierForTurn"], "default");
    }
}

#[test]
fn changing_host_tier_does_not_interrupt_or_steer_an_ongoing_turn() {
    let (workspace, fake) = tier_fixture();
    let path = workspace.path().join("tier.json");
    fs::write(&path, br#"{"service_tier":"fast"}"#).unwrap();
    let script = fs::read_to_string(&fake).unwrap().replace(
        "      if [ -e native-context-fail ]; then",
        "      touch tier-live-started\n      while [ ! -e tier-live-release ]; do sleep 0.01; done\n      if [ -e native-context-fail ]; then",
    );
    fs::write(&fake, script).unwrap();
    let adapter = codex_adapter(workspace.path()).with_environment("CWR_CODEX_SERVICE_TIER_FILE", &path);
    let mut request = native_text_invocation(&fake);
    request.adapter_params["model"] = json!("fixture-model");
    let marker = workspace.path().join("tier-live-started");
    let runner = thread::spawn(move || run_process_adapter(&adapter, request,
        &ProcessAdapterStopControl::new(), |_| {}));
    let started = wait_for_fixture_marker(&marker);
    // Always release the fixture, including an assertion failure.
    fs::write(&path, br#"{"service_tier":"default"}"#).unwrap();
    fs::write(workspace.path().join("tier-live-release"), "").unwrap();
    assert!(started);
    let outcome = runner.join().unwrap().unwrap();
    let ProcessAdapterOutcome::Completed(terminal) = outcome else { panic!("stopped"); };
    assert_eq!(terminal.status, 200);
    let requests = native_context_requests(workspace.path());
    let turns = requests.iter().filter(|r| r["method"] == "turn/start").collect::<Vec<_>>();
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0]["params"]["serviceTierForTurn"], "fast");
    assert!(requests.iter().all(|r| r["method"] != "turn/interrupt" && r["method"] != "turn/steer"));
}

#[test]
fn resumed_turn_stays_retryable_during_service_tier_lookup() {
    let (workspace, fake) = tier_fixture();
    let script = fs::read_to_string(&fake).unwrap().replace(
        "    *'\"method\":\"model/list\"'*)\n      if",
        "    *'\"method\":\"model/list\"'*)\n      touch tier-model-list-started\n      while [ ! -e tier-model-list-release ]; do sleep 0.01; done\n      if",
    );
    fs::write(&fake, script).unwrap();
    let preference = workspace.path().join("tier.json");
    fs::write(&preference, br#"{"service_tier":"default"}"#).unwrap();
    let adapter = codex_adapter(workspace.path())
        .with_environment("CWR_CODEX_SERVICE_TIER_FILE", &preference);
    let mut request = native_text_invocation(&fake);
    request.adapter_params["model"] = json!("fixture-model");
    request.conversation_continuity = true;
    request.thread["working_conversation"] =
        json!({"ref":"cwp://example.test/conversations/working"});
    let mut state = None;
    let outcome = run_process_adapter_with_activity_and_host_calls(
        &adapter,
        request.clone(),
        &ProcessAdapterStopControl::new(),
        |_| {},
        |activity| {
            if let AdapterActivityEvent::ProviderEvent { method, data, .. } = activity.event {
                if method == "cwr.working_state" {
                    state = Some(data);
                }
            }
            Ok(())
        },
        |_| panic!("no tools"),
    )
    .unwrap();
    assert!(matches!(outcome, ProcessAdapterOutcome::Completed(_)));

    fs::write(&preference, br#"{"service_tier":"fast"}"#).unwrap();
    request.invocation_ref = "cwp://example.test/invocations/next".into();
    request.thread["working_conversation"]["state"] = state.unwrap();
    let marker = workspace.path().join("tier-model-list-started");
    let release = workspace.path().join("tier-model-list-release");
    let stop = ProcessAdapterStopControl::new();
    let runner_stop = stop.clone();
    let (checkpoint_sender, checkpoint_receiver) = mpsc::channel();
    let runner = thread::spawn(move || {
        run_process_adapter_with_activity_and_host_calls(
            &adapter,
            request,
            &runner_stop,
            |_| {},
            |activity| {
                if let AdapterActivityEvent::ProviderEvent { method, data, .. } = activity.event {
                    if method == "cwr.adapter_checkpoint" {
                        checkpoint_sender.send(data).unwrap();
                    }
                }
                Ok(())
            },
            |_| panic!("no tools"),
        )
    });
    if !wait_for_fixture_marker(&marker) {
        stop.stop();
        let _ = fs::write(&release, b"release");
        panic!("service-tier lookup did not start");
    }
    let checkpoint = checkpoint_receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("history checkpoint before service-tier lookup");
    assert_eq!(checkpoint["live_started"], false);
    stop.stop();
    fs::write(&release, b"release").unwrap();
    assert_eq!(runner.join().unwrap().unwrap(), ProcessAdapterOutcome::Stopped);

    let requests = native_context_requests(workspace.path());
    let resume = requests
        .iter()
        .rposition(|request| request["method"] == "thread/resume")
        .unwrap();
    assert!(!requests[resume + 1..]
        .iter()
        .any(|request| request["method"] == "turn/start"));
}

#[test]
fn provider_default_model_uses_paginated_tiers_and_catalog_errors_fall_back() {
    let (workspace, fake) = tier_fixture();
    let path = workspace.path().join("tier.json");
    fs::write(&path, br#"{"service_tier":"fast"}"#).unwrap();
    let script = fs::read_to_string(&fake).unwrap()
        .replace(r#""result":{"thread":{"id":"native-context-thread"}}"#,
            r#""result":{"model":"fixture-model","thread":{"id":"native-context-thread"}}"#)
        .replace("      if [ -e unsupported-model ]; then", r#"      if [ -e catalog-error ]; then
        printf '%s\n' '{"id":"cwr-service-tier-models","error":{"code":-32601,"message":"unavailable"}}'
      elif [ -e unknown-model ]; then
        printf '%s\n' '{"id":"cwr-service-tier-models","result":{"data":[],"nextCursor":null}}'
      elif [ -e paged-model ] && echo "$line" | grep -q '"cursor":null'; then
        printf '%s\n' '{"id":"cwr-service-tier-models","result":{"data":[{"model":"other"}],"nextCursor":"page-two"}}'
      elif [ -e unsupported-model ]; then"#);
    fs::write(&fake, script).unwrap();
    for (mode, expected) in [("paged-model", "fast"), ("catalog-error", "default"), ("unknown-model", "default")] {
        let marker = workspace.path().join(mode);
        fs::write(&marker, "").unwrap();
        let adapter = codex_adapter(workspace.path()).with_environment("CWR_CODEX_SERVICE_TIER_FILE", &path);
        let mut request = native_text_invocation(&fake);
        request.adapter_params.as_object_mut().unwrap().remove("model");
        request.adapter_params.as_object_mut().unwrap().remove("reasoning_effort");
        let outcome = run_process_adapter(&adapter, request, &ProcessAdapterStopControl::new(), |_| {}).unwrap();
        let ProcessAdapterOutcome::Completed(terminal) = outcome else { panic!("stopped"); };
        assert_eq!(terminal.status, 200);
        let requests = native_context_requests(workspace.path());
        assert_eq!(requests.iter().rfind(|r| r["method"] == "turn/start").unwrap()["params"]["serviceTierForTurn"], expected);
        if mode == "paged-model" {
            assert!(requests.iter().any(|r| r["method"] == "model/list" && r["params"]["cursor"] == "page-two"));
        }
        fs::remove_file(marker).unwrap();
    }
}
