use std::path::Path;

use serde_json::{json, Value};

use super::*;

fn adapter_with_schema(path: &str) -> ResolvedAdapter {
    ResolvedAdapter {
        definition: AdapterFile {
            name: "Test adapter".into(),
            reference: "cwp://example.com/adapters/test".into(),
            authority_capabilities: Vec::new(),
            coworker_params_schema: None,
            integration_params_schema: None,
            actor_integration_params_schema: Some(SchemaLocator {
                path: Some(path.into()),
                reference: None,
            }),
        },
        effective_ref: "cwp://example.com/adapters/test".into(),
        source: AdapterSource::File(Path::new("adapter.toml").into()),
        schema_root_ref: None,
    }
}

#[test]
fn adapter_params_warn_but_accept_unsupported_properties() {
    let directory = tempfile::tempdir().unwrap();
    let adapter_path = directory.path().join("adapter.toml");
    let schema_path = directory.path().join("params.schema.json");
    std::fs::write(
        &schema_path,
        r#"{
            "type": "object",
            "additionalProperties": false,
            "properties": {"supported": {"type": "boolean"}}
        }"#,
    )
    .unwrap();
    let mut adapter = adapter_with_schema("./params.schema.json");
    adapter.source = AdapterSource::File(adapter_path.clone());

    validate_adapter_params(
        &adapter_path,
        "actor integration parameters",
        &adapter,
        adapter.definition.actor_integration_params_schema.as_ref(),
        &json!({"unsupported": true}).into(),
        &EmptyResourceResolver,
    )
    .unwrap();
}

#[test]
fn adapter_params_still_reject_invalid_supported_properties() {
    let directory = tempfile::tempdir().unwrap();
    let adapter_path = directory.path().join("adapter.toml");
    let schema_path = directory.path().join("params.schema.json");
    std::fs::write(
        &schema_path,
        r#"{
            "type": "object",
            "additionalProperties": false,
            "properties": {"supported": {"type": "boolean"}}
        }"#,
    )
    .unwrap();
    let mut adapter = adapter_with_schema("./params.schema.json");
    adapter.source = AdapterSource::File(adapter_path.clone());

    let error = validate_adapter_params(
        &adapter_path,
        "actor integration parameters",
        &adapter,
        adapter.definition.actor_integration_params_schema.as_ref(),
        &json!({"supported": "not a boolean", "unsupported": true}).into(),
        &EmptyResourceResolver,
    )
    .unwrap_err();

    assert!(error.to_string().contains("/supported"), "{error}");
}

#[test]
fn loopback_host_calls_validate_before_adapter_execution() {
    let directory = tempfile::tempdir().unwrap();
    let adapter_path = directory.path().join("adapter.toml");
    std::fs::write(
        directory.path().join("params.schema.json"),
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../packages/cwr-core/contracts/cwr/adapters/2026-09/loopback.coworker-params.schema.json"
        )),
    )
    .unwrap();
    let mut adapter = adapter_with_schema("./params.schema.json");
    adapter.source = AdapterSource::File(adapter_path.clone());
    let validate = |params: Value| {
        validate_adapter_params(
            &adapter_path,
            "coworker parameters",
            &adapter,
            adapter.definition.actor_integration_params_schema.as_ref(),
            &params.into(),
            &EmptyResourceResolver,
        )
    };
    for calls in [
        json!([]),
        json!([
            {"operation": "cwr.list_actions", "input": {}},
            {"operation": "cwr.list_coworkers", "input": {}}
        ]),
    ] {
        validate(json!({"host_calls": calls})).unwrap();
    }
    for calls in [
        json!("invalid"),
        json!(null),
        json!([1]),
        json!([{"input": {}}]),
        json!([{"operation": "cwr.list_actions"}]),
        json!([{"operation": 1, "input": {}}]),
        json!([{"operation": "", "input": {}}]),
    ] {
        let error = validate(json!({"host_calls": calls})).unwrap_err();
        assert!(error.to_string().contains("/host_calls"), "{error}");
    }
    // The singular parameter retains its existing validation and shape.
    validate(json!({"host_call": {"operation": "cwr.list_actions", "input": {}}})).unwrap();
}

fn terminal_invocation(state: &str) -> Value {
    let response = match state {
        "completed" => json!({
            "invocation_ref": "~@/invocations/terminal",
            "sequence": 1,
            "status": 200,
            "summary": "Completed",
            "result": {}
        }),
        "blocked" => json!({
            "invocation_ref": "~@/invocations/terminal",
            "sequence": 1,
            "status": 422,
            "summary": "Blocked",
            "error": {"code": "blocked"}
        }),
        "failed" => json!({
            "invocation_ref": "~@/invocations/terminal",
            "sequence": 1,
            "status": 500,
            "summary": "Failed",
            "error": {"code": "failed"}
        }),
        "stopped" => json!({
            "invocation_ref": "~@/invocations/terminal",
            "sequence": 1,
            "status": 499,
            "summary": "Stopped"
        }),
        _ => unreachable!("test only constructs terminal invocation states"),
    };
    json!({
        "type": ACTION_INVOCATION_TYPE,
        "ref": "~@/invocations/terminal",
        "requester_actor_ref": "~@/actors/requester",
        "target_actor_ref": "~@/actors/target",
        "action_name": "draft--brief",
        "input": {},
        "state": state,
        "responses": [response]
    })
}

#[test]
fn concrete_coworker_accepts_no_roles_and_rejects_old_subject_fields() {
    let source = Path::new("COWORKER.toml");
    let mut coworker = json!({"type": COWORKER_TYPE, "name": "Person"});
    validate_core_document(source, COWORKER_TYPE, &coworker).unwrap();
    coworker["ref"] = json!("~@/coworkers/person");
    assert!(validate_core_document(source, COWORKER_TYPE, &coworker).is_err());
}

#[test]
fn execution_terminal_states_require_actor_integration_but_stopped_does_not() {
    let source = Path::new("action_invocation.json");
    for state in ["completed", "blocked", "failed"] {
        let mut invocation = terminal_invocation(state);
        assert!(
            validate_core_document(source, ACTION_INVOCATION_TYPE, &invocation).is_err(),
            "{state} without actor_integration_ref must be rejected"
        );
        invocation["actor_integration_ref"] = json!("~@/actor_integrations/target");
        validate_core_document(source, ACTION_INVOCATION_TYPE, &invocation).unwrap();
    }

    validate_core_document(
        source,
        ACTION_INVOCATION_TYPE,
        &terminal_invocation("stopped"),
    )
    .unwrap();
}

#[test]
fn stopped_state_requires_actor_integration_after_execution_starts() {
    let source = Path::new("action_invocation.json");

    let mut started = terminal_invocation("stopped");
    started["started_at"] = json!("2026-08-27T12:00:00Z");
    assert!(
        validate_core_document(source, ACTION_INVOCATION_TYPE, &started).is_err(),
        "stopped invocation with started_at must retain actor_integration_ref"
    );
    started["actor_integration_ref"] = json!("~@/actor_integrations/target");
    validate_core_document(source, ACTION_INVOCATION_TYPE, &started).unwrap();

    let mut progressed = terminal_invocation("stopped");
    progressed["responses"] = json!([
        {
            "invocation_ref": "~@/invocations/terminal",
            "sequence": 1,
            "status": 102,
            "summary": "Started work"
        },
        {
            "invocation_ref": "~@/invocations/terminal",
            "sequence": 2,
            "status": 499,
            "summary": "Stopped"
        }
    ]);
    assert!(
        validate_core_document(source, ACTION_INVOCATION_TYPE, &progressed).is_err(),
        "stopped invocation with progress must retain actor_integration_ref"
    );
    progressed["actor_integration_ref"] = json!("~@/actor_integrations/target");
    validate_core_document(source, ACTION_INVOCATION_TYPE, &progressed).unwrap();
}

#[test]
fn action_invocation_schema_accepts_noncanonical_progress_and_completion_codes() {
    let source = Path::new("action_invocation.json");
    let mut completed = terminal_invocation("completed");
    completed["actor_integration_ref"] = json!("~@/actor_integrations/target");
    completed["responses"][0]["status"] = json!(202);
    completed["responses"][0]["result"] = Value::Null;
    validate_core_document(source, ACTION_INVOCATION_TYPE, &completed).unwrap();

    let mut running = terminal_invocation("completed");
    running["state"] = json!("running");
    running["actor_integration_ref"] = json!("~@/actor_integrations/target");
    running["responses"] = json!([{
        "invocation_ref": "~@/invocations/terminal",
        "sequence": 1,
        "status": 150,
        "summary": "Working"
    }]);
    validate_core_document(source, ACTION_INVOCATION_TYPE, &running).unwrap();
}

#[test]
fn action_invocation_retains_timeline_source_and_decision_together() {
    let source = Path::new("action_invocation.json");
    let mut invocation = terminal_invocation("completed");
    invocation["actor_integration_ref"] = json!("~@/actor_integrations/target");
    invocation["conversation_ref"] = json!("~@/conversations/shared");
    invocation["source_message_ref"] = json!("~@/messages/source");
    assert!(validate_core_document(source, ACTION_INVOCATION_TYPE, &invocation).is_err());

    invocation["timeline_publications"] = json!({
        "cwp://example.test/publications/invocation": {"kind": "root"},
        "cwp://example.test/publications/invocation-again": {"kind": "undisclosed"}
    });
    validate_core_document(source, ACTION_INVOCATION_TYPE, &invocation).unwrap();

    for invalid_ref in [
        "~@/publications/invocation",
        "cwp://example.test/publications/invocation#member",
    ] {
        invocation["timeline_publications"] = json!({
            (invalid_ref): {"kind": "root"}
        });
        assert!(validate_core_document(source, ACTION_INVOCATION_TYPE, &invocation).is_err());
    }
    invocation["timeline_publications"] = json!({
        "cwp://example.test/publications/invocation": {"kind": "root"}
    });

    for invalid_parent_ref in [
        "~@/timeline/items/relative",
        "cwp://example.test/timeline/items/fragmented#member",
    ] {
        invocation["timeline_publications"] = json!({
            "cwp://example.test/publications/child": {
                "kind": "child",
                "parent_item_ref": invalid_parent_ref
            }
        });
        assert!(validate_core_document(source, ACTION_INVOCATION_TYPE, &invocation).is_err());
    }
    invocation["timeline_publications"] = json!({
        "cwp://example.test/publications/invocation": {"kind": "root"}
    });

    let conversation_ref = invocation
        .as_object_mut()
        .unwrap()
        .remove("conversation_ref")
        .unwrap();
    assert!(validate_core_document(source, ACTION_INVOCATION_TYPE, &invocation).is_err());
    invocation["conversation_ref"] = conversation_ref;

    invocation
        .as_object_mut()
        .unwrap()
        .remove("source_message_ref");
    assert!(validate_core_document(source, ACTION_INVOCATION_TYPE, &invocation).is_err());
}

#[test]
fn explicit_timeline_examples_satisfy_the_core_contracts() {
    const CASES: &[(&str, &str)] = &[
        (
            "cwp://org.coworkerprotocol/2026-09/schemas/conversation_timeline_content",
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../packages/cwr-core/contracts/cwp/schemas/2026-09/conversation_timeline_content/example.json"
            )),
        ),
        (
            "cwp://org.coworkerprotocol/2026-09/schemas/conversation_timeline_cursor",
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../packages/cwr-core/contracts/cwp/schemas/2026-09/conversation_timeline_cursor/example.json"
            )),
        ),
        (
            "cwp://org.coworkerprotocol/2026-09/schemas/conversation_timeline_item",
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../packages/cwr-core/contracts/cwp/schemas/2026-09/conversation_timeline_item/example.json"
            )),
        ),
        (
            "cwp://org.coworkerprotocol/2026-09/schemas/conversation_timeline_connection",
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../packages/cwr-core/contracts/cwp/schemas/2026-09/conversation_timeline_connection/example.json"
            )),
        ),
    ];
    for (schema_ref, document) in CASES {
        let document = serde_json::from_str(document).unwrap();
        validate_core_document(Path::new("timeline-example.json"), schema_ref, &document)
            .unwrap_or_else(|error| panic!("{schema_ref}: {error}"));
    }

    let item_schema = "cwp://org.coworkerprotocol/2026-09/schemas/conversation_timeline_item";
    let mut invocation_item: Value = serde_json::from_str(CASES[2].1).unwrap();
    invocation_item
        .as_object_mut()
        .unwrap()
        .remove("child_connection_ref");
    assert!(validate_core_document(
        Path::new("timeline-item.json"),
        item_schema,
        &invocation_item
    )
    .is_err());

    let mut message_item: Value = serde_json::from_str(CASES[2].1).unwrap();
    message_item["content"] = json!({
        "kind": "message",
        "message_ref": "cwp://com.example.runtime/messages/01JEXAMPLE"
    });
    assert!(
        validate_core_document(Path::new("timeline-item.json"), item_schema, &message_item)
            .is_err()
    );

    let cursor_schema = "cwp://org.coworkerprotocol/2026-09/schemas/conversation_timeline_cursor";
    let mut cursor: Value = serde_json::from_str(CASES[1].1).unwrap();
    cursor["version"] = json!(2);
    assert!(
        validate_core_document(Path::new("timeline-cursor.json"), cursor_schema, &cursor).is_err()
    );
}
