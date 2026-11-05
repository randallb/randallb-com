fn fixture(script: &str) -> (TempDir, PathBuf) {
    let directory = tempfile::tempdir().expect("create adapter fixture");
    fs::create_dir(directory.path().join("home")).expect("create fake home");
    fs::write(
        directory.path().join("PROMPT.md"),
        "You are the fixture coworker.\n",
    )
    .expect("write fixture prompt");
    let executable = directory.path().join("codex");
    fs::write(&executable, script).expect("write fake Codex executable");
    let mut permissions = fs::metadata(&executable)
        .expect("read fake Codex metadata")
        .permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&executable, permissions).expect("make fake Codex executable");
    (directory, executable)
}

fn wait_for_fixture_marker(path: &Path) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !path.exists() {
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(10));
    }
    true
}

fn codex_adapter(directory: &Path) -> ResolvedProcessAdapter {
    ResolvedProcessAdapter::new(PathBuf::from(env!("CARGO_BIN_EXE_cwr-adapter-codex")))
        .with_current_directory(directory)
        .with_environment("CWR_COWORKER_DIRECTORY", directory)
        .with_environment("CWR_CODEX_DANGER_FULL_ACCESS_WORKSPACE", directory)
        .with_environment("HOME", directory.join("home"))
        .with_environment("PATH", "/usr/bin:/bin")
        .with_environment("USER", "fixture-user")
}
fn invocation(codex_executable: &Path) -> AdapterInvocation {
    let mut invocation = AdapterInvocation::new(
        "cwp://org.coworkerprotocol/2026-09/invocations/codex-test",
        "test--action",
        json!({"message": "please work"}),
        json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["message"],
            "properties": {"message": {"type": "string"}}
        }),
    );
    invocation.model_run_ref =
        Some("cwp://org.coworkerprotocol/2026-09/model_runs/codex-test".into());
    invocation.integration_ref = Some("cwp://com.example/integrations/codex".into());
    invocation.thread = json!({
        "messages": [{"sender": "@randall", "text": "please work"}],
        "participants": [{"handle": "@helper"}]
    });
    invocation.adapter_params = json!({
        "entrypoint": "./PROMPT.md",
        "model": "gpt-5.6-sol",
        "reasoning_effort": "low"
    });
    invocation.integration_params = json!({
        "codex_executable": codex_executable
    });
    invocation
}
fn native_text_invocation(codex_executable: &Path) -> AdapterInvocation {
    let mut invocation = invocation(codex_executable);
    invocation.action_name = "respond_to--message".into();
    invocation.output_schema = json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["text"],
        "properties": {"text": {"type": "string", "minLength": 1}}
    });
    invocation
}
fn send_message_operation() -> AdapterHostOperation {
    AdapterHostOperation {
        name: "cwr.send_message".into(),
        description: "Address one participant and wait for the reply.".into(),
        input_schema: json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["to", "text"],
            "properties": {
                "to": {"type": "string", "enum": ["@helper"]},
                "text": {"type": "string", "minLength": 1}
            }
        }),
        output_schema: json!({}),
    }
}
fn invoke_action_operation() -> AdapterHostOperation {
    AdapterHostOperation {
        name: "cwr.invoke_action".into(),
        description: "Invoke a visible coworker action and wait for its result.".into(),
        input_schema: json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["activation", "input"],
            "properties": {
                "activation": {"const": "@rng--roll--dice"},
                "input": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["count", "sides", "modifier"],
                    "properties": {
                        "count": {"type": "integer", "minimum": 1},
                        "sides": {"type": "integer", "minimum": 2},
                        "modifier": {"type": "integer"}
                    }
                }
            }
        }),
        output_schema: json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["notation", "rolls", "modifier", "total", "text"],
            "properties": {
                "notation": {"type": "string"},
                "rolls": {"type": "array", "items": {"type": "integer"}},
                "modifier": {"type": "integer"},
                "total": {"type": "integer"},
                "text": {"type": "string"}
            }
        }),
    }
}
fn dynamic_operation(
    name: &str,
    description: &str,
    input_schema: serde_json::Value,
    output_schema: serde_json::Value,
) -> AdapterHostOperation {
    AdapterHostOperation {
        name: name.into(),
        description: description.into(),
        input_schema,
        output_schema,
    }
}
