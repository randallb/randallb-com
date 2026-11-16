use super::*;

#[test]
fn selective_history_requires_host_tools() {
    let params: CodexAdapterParams = serde_json::from_value(json!({
        "entrypoint": "./PROMPT.md",
        "isolated": true,
        "selective_history": true
    }))
    .unwrap();
    assert!(validate_codex_history_mode(&params)
        .unwrap_err()
        .to_string()
        .contains("isolated mode disables them"));
}

#[test]
fn selective_history_defaults_on_but_explicit_false_and_isolation_disable_it() {
    for (extra, enabled) in [
        (json!({}), true),
        (json!({"selective_history":true}), true),
        (json!({"selective_history":false}), false),
        (json!({"isolated":true}), false),
    ] {
        let mut params = json!({"entrypoint":"./PROMPT.md"});
        params
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let params: CodexAdapterParams = serde_json::from_value(params).unwrap();
        validate_codex_history_mode(&params).unwrap();
        assert_eq!(
            params.selective_history.unwrap_or(true) && !params.isolated,
            enabled
        );
    }
}

fn host_operation(name: &str) -> AdapterHostOperation {
    AdapterHostOperation {
        name: name.into(),
        description: "Test host operation.".into(),
        input_schema: json!({"type": "object"}),
        output_schema: json!({"type": "object"}),
    }
}

#[test]
fn duplicate_dynamic_host_tool_names_are_rejected() {
    let error = codex_host_tools(&[
        host_operation("cwr.start_action"),
        host_operation("cwr.start_action"),
    ])
    .expect_err("duplicate host tools must fail");
    assert!(error.to_string().contains("declared more than once"));
}

#[test]
fn distinct_host_ids_cannot_collide_on_one_cvo_name() {
    assert!(codex_host_tools(&[
        host_operation("cwr.inspect_invocation"),
        host_operation("cwr.debug_invocation"),
    ])
    .unwrap_err()
    .to_string()
    .contains("declared more than once"));
}

#[test]
fn adapter_owned_dynamic_tool_names_are_rejected() {
    for name in ["cwp.cwr--return--result", "cwr.observe_thread"] {
        let error = codex_host_tools(&[host_operation(name)])
            .expect_err("adapter-owned host tool must fail");
        assert!(!error.to_string().is_empty());
    }
}

#[test]
fn empty_entrypoint_is_valid_developer_instructions() {
    let directory = tempfile::tempdir().expect("temporary workspace");
    fs::write(directory.path().join("PROMPT.md"), "").expect("empty entrypoint");

    let instructions =
        read_entrypoint(directory.path(), "./PROMPT.md").expect("read empty entrypoint");

    assert!(instructions.is_empty());
}

#[test]
fn respond_to_message_requests_native_assistant_text_without_exposing_the_output_schema() {
    let mut invocation = AdapterInvocation::new(
        "cwp://com.example/invocations/message",
        RESPOND_TO_MESSAGE_ACTION,
        json!({"message": "hello"}),
        json!({
            "type": "object",
            "required": ["text"],
            "properties": {"text": {"type": "string"}}
        }),
    );

    let observation = build_observation(&mut invocation, true).expect("build observation");

    assert_eq!(
        observation["action_request"]["return_contract"],
        json!({"type": "assistant_text"})
    );
}
