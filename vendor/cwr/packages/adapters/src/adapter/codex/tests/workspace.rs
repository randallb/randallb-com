#[test]
fn codex_uses_execution_cwd_but_reads_prompt_from_coworker_directory() {
    let script = r#"#!/bin/sh
pwd -P > process-cwd
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*)
      printf '%s\n' '{"id":"cwr-initialize","result":{}}' ;;
    *'"method":"thread/start"'*|*'"method":"thread/resume"'*)
      printf '%s\n' "$line" > thread-request
      printf '%s\n' '{"id":"cwr-thread-start","result":{"thread":{"id":"cwd-thread"}}}' ;;
    *'"method":"thread/inject_items"'*)
      printf '%s\n' '{"id":"cwr-observation-inject","result":{}}' ;;
    *'"method":"turn/start"'*)
      printf '%s\n' '{"id":"cwr-turn-start","result":{"turn":{"id":"cwd-turn"}}}'
      printf '%s\n' '{"id":2,"method":"item/tool/call","params":{"namespace":"cwp","tool":"cwr--return--result","arguments":{"status":200,"summary":"Done.","result":{"message":"done"}}}}' ;;
    *'"id":2'*'"result"'*)
      printf '%s\n' '{"method":"turn/completed","params":{"threadId":"cwd-thread","turn":{"id":"cwd-turn","status":"completed"}}}' ;;
  esac
done
"#;
    let (directory, fake_codex) = fixture(script);
    let execution = directory.path().join("project");
    fs::create_dir(&execution).unwrap();
    fs::write(
        execution.join("PROMPT.md"),
        "Wrong prompt from execution cwd",
    )
    .unwrap();
    let adapter = codex_adapter(directory.path())
        .with_current_directory(&execution)
        .with_environment("CWR_CODEX_DANGER_FULL_ACCESS_WORKSPACE", &execution);
    {
        let requested = invocation(&fake_codex);
        let outcome = run_process_adapter(
            &adapter,
            requested,
            &ProcessAdapterStopControl::new(),
            |_| {},
        )
        .unwrap();
        assert!(
            matches!(outcome, ProcessAdapterOutcome::Completed(terminal) if terminal.status == 200)
        );
        let request: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(execution.join("thread-request")).unwrap())
                .unwrap();
        assert_eq!(
            request["params"]["cwd"],
            json!(execution.canonicalize().unwrap())
        );
        assert_eq!(
            request["params"]["developerInstructions"],
            "You are the fixture coworker.\n"
        );
        assert_eq!(request["params"]["sandbox"], "danger-full-access");
        assert_eq!(
            fs::read_to_string(execution.join("process-cwd"))
                .unwrap()
                .trim(),
            execution.canonicalize().unwrap().to_str().unwrap()
        );
    }
    let entrypoint = directory.path().join("PROMPT.md");
    for remove_file in [false, true] {
        if remove_file { fs::remove_file(&entrypoint).unwrap(); }
        else { fs::write(&entrypoint, "Unactivated disk edit").unwrap(); }
        let mut requested = invocation(&fake_codex);
        requested.adapter_params["role_contexts"] = json!([]);
        requested.adapter_params["activated_entrypoints"] = json!([entrypoint]);
        requested.adapter_params["activated_sources"] = json!({
            (entrypoint.to_string_lossy().into_owned()): "Admitted instructions A"
        });
        run_process_adapter(&adapter, requested, &ProcessAdapterStopControl::new(), |_| {}).unwrap();
        let request: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(execution.join("thread-request")).unwrap()).unwrap();
        assert_eq!(request["params"]["developerInstructions"], "Admitted instructions A");
    }
    fs::remove_file(execution.join("process-cwd")).unwrap();
    let mut requested = invocation(&fake_codex);
    requested.adapter_params["role_contexts"] = json!([]);
    requested.adapter_params["activated_entrypoints"] = json!([entrypoint]);
    requested.adapter_params["activated_sources"] = json!({});
    let error = run_process_adapter(&adapter, requested, &ProcessAdapterStopControl::new(), |_| {}).unwrap_err();
    assert!(error.to_string().contains("before returning a result"));
    assert!(!execution.join("process-cwd").exists(), "missing activated sources must reject before starting the provider");

}

#[test]
fn workspace_write_is_an_explicit_opt_down() {
    let script = r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*)
      printf '%s\n' '{"id":"cwr-initialize","result":{}}' ;;
    *'"method":"thread/start"'*)
      printf '%s\n' "$line" > thread-request
      printf '%s\n' '{"id":"cwr-thread-start","result":{"thread":{"id":"opt-down-thread"}}}' ;;
    *'"method":"thread/inject_items"'*)
      printf '%s\n' '{"id":"cwr-observation-inject","result":{}}' ;;
    *'"method":"turn/start"'*)
      printf '%s\n' '{"id":"cwr-turn-start","result":{"turn":{"id":"opt-down-turn"}}}'
      printf '%s\n' '{"id":2,"method":"item/tool/call","params":{"namespace":"cwp","tool":"cwr--return--result","arguments":{"status":200,"summary":"Done.","result":{"message":"done"}}}}' ;;
    *'"id":2'*'"result"'*)
      printf '%s\n' '{"method":"turn/completed","params":{"threadId":"opt-down-thread","turn":{"id":"opt-down-turn","status":"completed"}}}' ;;
  esac
done
"#;
    let (directory, fake_codex) = fixture(script);
    let mut requested = invocation(&fake_codex);
    requested.actor_integration_params = json!({"sandbox": "workspace-write"});

    run_process_adapter(
        &codex_adapter(directory.path()),
        requested,
        &ProcessAdapterStopControl::new(),
        |_| {},
    )
    .unwrap();

    let request: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(directory.path().join("thread-request")).unwrap(),
    )
    .unwrap();
    assert_eq!(request["params"]["sandbox"], "workspace-write");
}

#[test]
fn isolated_codex_actions_are_read_only_offline_and_receive_no_host_tools() {
    let script = r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*)
      printf '%s\n' '{"id":"cwr-initialize","result":{}}' ;;
    *'"method":"thread/start"'*)
      printf '%s\n' "$line" > thread-request
      printf '%s\n' '{"id":"cwr-thread-start","result":{"thread":{"id":"isolated-thread"}}}' ;;
    *'"method":"thread/inject_items"'*)
      printf '%s\n' '{"id":"cwr-observation-inject","result":{}}' ;;
    *'"method":"turn/start"'*)
      printf '%s\n' '{"id":"cwr-turn-start","result":{"turn":{"id":"isolated-turn"}}}'
      printf '%s\n' '{"id":2,"method":"item/tool/call","params":{"namespace":"cwp","tool":"cwr--return--result","arguments":{"status":200,"summary":"Done.","result":{"message":"done"}}}}' ;;
    *'"id":2'*'"result"'*)
      printf '%s\n' '{"method":"turn/completed","params":{"threadId":"isolated-thread","turn":{"id":"isolated-turn","status":"completed"}}}' ;;
  esac
done
"#;
    let (directory, fake_codex) = fixture(script);
    let mut requested = invocation(&fake_codex);
    requested.adapter_params["isolated"] = json!(true);
    requested.actor_integration_params = json!({"sandbox": "danger-full-access"});
    requested.host_operations.push(send_message_operation());

    run_process_adapter(
        &codex_adapter(directory.path()),
        requested,
        &ProcessAdapterStopControl::new(),
        |_| {},
    )
    .unwrap();

    let request: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(directory.path().join("thread-request")).unwrap(),
    )
    .unwrap();
    assert_eq!(request["params"]["sandbox"], "read-only");
    assert_eq!(
        request["params"]["config"]["sandbox_workspace_write"]["network_access"],
        false
    );
    assert_eq!(
        request["params"]["dynamicTools"][0]["tools"]
            .as_array()
            .unwrap()
            .len(),
        1,
        "isolated actions expose only their validated return_result tool"
    );
}

#[test]
fn cwr_only_codex_is_read_only_offline_and_retains_host_tools() {
    let script = r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*)
      printf '%s\n' '{"id":"cwr-initialize","result":{}}' ;;
    *'"method":"thread/start"'*)
      printf '%s\n' "$line" > thread-request
      printf '%s\n' '{"id":"cwr-thread-start","result":{"thread":{"id":"restricted-thread"}}}' ;;
    *'"method":"thread/inject_items"'*)
      printf '%s\n' '{"id":"cwr-observation-inject","result":{}}' ;;
    *'"method":"turn/start"'*)
      printf '%s\n' '{"id":"cwr-turn-start","result":{"turn":{"id":"restricted-turn"}}}'
      printf '%s\n' '{"id":2,"method":"item/tool/call","params":{"namespace":"cwp","tool":"cwr--return--result","arguments":{"status":200,"summary":"Done.","result":{"message":"done"}}}}' ;;
    *'"id":2'*'"result"'*)
      printf '%s\n' '{"method":"turn/completed","params":{"threadId":"restricted-thread","turn":{"id":"restricted-turn","status":"completed"}}}' ;;
  esac
done
"#;
    let (directory, fake_codex) = fixture(script);
    let mut requested = invocation(&fake_codex);
    requested.actor_integration_params =
        json!({
            "cwr_only": true,
            "cwr_action_actor_refs": ["~@/actors/feedback_ops"],
            "sandbox": "danger-full-access"
        });
    let mut operation = invoke_action_operation();
    operation.description = "Invoke one visible coworker action.\n@workloop_ops--open--workspace: Show a folder picker only to select another workspace.".into();
    let expected_description = operation.description.clone();
    requested.host_operations.push(operation);

    run_process_adapter(
        &codex_adapter(directory.path()),
        requested,
        &ProcessAdapterStopControl::new(),
        |_| {},
    )
    .unwrap();

    let request: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(directory.path().join("thread-request")).unwrap(),
    )
    .unwrap();
    let invoke_tool = request["params"]["dynamicTools"][0]["tools"].as_array().unwrap()
        .iter().find(|tool| tool["name"] == "cwr--invoke--action").unwrap();
    assert_eq!(invoke_tool["description"], expected_description);
    assert_eq!(request["params"]["sandbox"], serde_json::Value::Null);
    assert_eq!(request["params"]["permissions"], "cwr-only");
    assert_eq!(
        request["params"]["config"]["permissions.cwr-only.network.enabled"],
        false
    );
    assert_eq!(request["params"]["config"]["features.shell_tool"], false);
    assert_eq!(request["params"]["config"]["features.unified_exec"], false);
    assert_eq!(
        request["params"]["dynamicTools"][0]["tools"]
            .as_array()
            .unwrap()
            .len(),
        2,
        "cwr-only actions retain return_result and CWR host tools"
    );
}

#[test]
fn cwr_only_codex_accepts_empty_action_permissions() {
    let script = r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*)
      printf '%s\n' '{"id":"cwr-initialize","result":{}}' ;;
    *'"method":"thread/start"'*)
      printf '%s\n' "$line" > thread-request
      printf '%s\n' '{"id":"cwr-thread-start","result":{"thread":{"id":"restricted-thread"}}}' ;;
    *'"method":"thread/inject_items"'*)
      printf '%s\n' '{"id":"cwr-observation-inject","result":{}}' ;;
    *'"method":"turn/start"'*)
      printf '%s\n' '{"id":"cwr-turn-start","result":{"turn":{"id":"restricted-turn"}}}'
      printf '%s\n' '{"id":2,"method":"item/tool/call","params":{"namespace":"cwp","tool":"cwr--return--result","arguments":{"status":200,"summary":"Done.","result":{"message":"done"}}}}' ;;
    *'"id":2'*'"result"'*)
      printf '%s\n' '{"method":"turn/completed","params":{"threadId":"restricted-thread","turn":{"id":"restricted-turn","status":"completed"}}}' ;;
  esac
done
"#;
    let (directory, fake_codex) = fixture(script);
    let mut requested = invocation(&fake_codex);
    requested.actor_integration_params =
        json!({
            "cwr_only": true,
            "cwr_action_actor_refs": [],
            "sandbox": "danger-full-access"
        });

    run_process_adapter(
        &codex_adapter(directory.path()),
        requested,
        &ProcessAdapterStopControl::new(),
        |_| {},
    )
    .unwrap();

    let request: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(directory.path().join("thread-request")).unwrap(),
    )
    .unwrap();
    assert_eq!(request["params"]["sandbox"], serde_json::Value::Null);
    assert_eq!(request["params"]["permissions"], "cwr-only");
    assert_eq!(
        request["params"]["config"]["permissions.cwr-only.network.enabled"],
        false
    );
    assert_eq!(request["params"]["config"]["features.shell_tool"], false);
    assert_eq!(request["params"]["config"]["features.unified_exec"], false);
    assert_eq!(
        request["params"]["dynamicTools"][0]["tools"]
            .as_array()
            .unwrap()
            .len(),
        1,
        "empty action permissions expose only return_result"
    );
}

#[test]
fn danger_full_access_requires_a_separate_host_grant() {
    let (directory, fake_codex) = fixture(FAKE_RESUME_CODEX);
    let mut requested = invocation(&fake_codex);
    requested.continuation = Some(json!("prior-thread"));

    let error = run_process_adapter(
        &codex_adapter(directory.path()).with_environment(
            "CWR_CODEX_DANGER_FULL_ACCESS_WORKSPACE",
            directory.path().join("home"),
        ),
        requested,
        &ProcessAdapterStopControl::new(),
        |_| {},
    )
    .unwrap_err();
    let failure = error
        .downcast_ref::<ProcessAdapterFailure>()
        .expect("adapter failure");
    assert!(failure
        .stderr_tail
        .contains("CWR_CODEX_DANGER_FULL_ACCESS_WORKSPACE"));
}

#[test]
fn danger_full_access_runs_with_a_host_grant() {
    let (directory, fake_codex) = fixture(FAKE_NUMERIC_STATUS_CODEX);
    let requested = invocation(&fake_codex);

    let outcome = run_process_adapter(
        &codex_adapter(directory.path()).with_environment(
            "CWR_CODEX_DANGER_FULL_ACCESS_WORKSPACE",
            directory.path(),
        ),
        requested,
        &ProcessAdapterStopControl::new(),
        |_| {},
    )
    .unwrap();
    assert!(matches!(outcome, ProcessAdapterOutcome::Completed(_)));
}

#[test]
fn codex_entrypoint_cannot_escape_definition_directory_into_workspace() {
    let (directory, fake_codex) = fixture(FAKE_RESUME_CODEX);
    let definition = directory.path().join("coworker");
    fs::create_dir(&definition).unwrap();
    std::os::unix::fs::symlink(
        directory.path().join("PROMPT.md"),
        definition.join("linked.md"),
    )
    .unwrap();
    let adapter =
        codex_adapter(directory.path()).with_environment("CWR_COWORKER_DIRECTORY", &definition);
    for entrypoint in ["./../PROMPT.md", "./PROMPT.md", "./linked.md"] {
        let mut requested = invocation(&fake_codex);
        requested.adapter_params["entrypoint"] = json!(entrypoint);
        assert!(run_process_adapter(
            &adapter,
            requested,
            &ProcessAdapterStopControl::new(),
            |_| {}
        )
        .is_err());
    }
}

#[test]
fn isolated_cwr_only_actions_have_no_native_files_shell_network_or_host_tools() {
    let script = r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*)
      printf '%s\n' '{"id":"cwr-initialize","result":{}}' ;;
    *'"method":"thread/start"'*)
      printf '%s\n' "$line" > thread-request
      printf '%s\n' '{"id":"cwr-thread-start","result":{"thread":{"id":"isolated-thread"}}}' ;;
    *'"method":"thread/inject_items"'*)
      printf '%s\n' '{"id":"cwr-observation-inject","result":{}}' ;;
    *'"method":"turn/start"'*)
      printf '%s\n' '{"id":"cwr-turn-start","result":{"turn":{"id":"isolated-turn"}}}'
      printf '%s\n' '{"id":2,"method":"item/tool/call","params":{"namespace":"cwp","tool":"cwr--return--result","arguments":{"status":200,"summary":"Done.","result":{"message":"done"}}}}' ;;
    *'"id":2'*'"result"'*)
      printf '%s\n' '{"method":"turn/completed","params":{"threadId":"isolated-thread","turn":{"id":"isolated-turn","status":"completed"}}}' ;;
  esac
done
"#;
    let (directory, fake_codex) = fixture(script);
    let mut requested = invocation(&fake_codex);
    requested.adapter_params["isolated"] = json!(true);
    requested.actor_integration_params =
        json!({"sandbox": "workspace-write", "cwr_only": true, "cwr_action_actor_refs": []});
    requested.host_operations.push(send_message_operation());

    run_process_adapter(
        &codex_adapter(directory.path()),
        requested,
        &ProcessAdapterStopControl::new(),
        |_| {},
    )
    .unwrap();

    let request: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(directory.path().join("thread-request")).unwrap())
            .unwrap();
    assert!(request["params"].get("sandbox").is_none());
    assert_eq!(request["params"]["permissions"], "cwr-only");
    let config = &request["params"]["config"];
    assert_eq!(config["permissions.cwr-only.filesystem"], json!({}));
    assert_eq!(config["permissions.cwr-only.network.enabled"], false);
    assert_eq!(config["features.shell_tool"], false);
    assert_eq!(config["features.unified_exec"], false);
    assert_eq!(config["web_search"], "disabled");
    assert_eq!(config["features.apps"], false);
    assert_eq!(
        request["params"]["dynamicTools"][0]["tools"]
            .as_array()
            .unwrap()
            .len(),
        1,
        "isolated actions expose only their validated return_result tool"
    );
}
