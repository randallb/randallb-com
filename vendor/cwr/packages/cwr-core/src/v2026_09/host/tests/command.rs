#[test]
fn continue_command_and_receipt_validate_and_round_trip() {
    assert_create_command_round_trip();

    let command = HostCommand::ContinueConversation {
        version: HOST_COMMAND_VERSION,
        request_id: "desktop-1".into(),
        source_ref: SOURCE_REF.into(),
        conversation_ref: format!("{RUNTIME_REF}/conversations/one"),
        author_actor_ref: crate::v2026_09::test_fixture::actor_ref("person").into(),
        text: "Continue here".into(),
        recovery_only: true,
    };
    command.validate().expect("validate continue command");
    let wire = serde_json::to_value(&command).expect("serialize continue command");
    assert_eq!(wire["kind"], "continue_conversation");
    assert_eq!(wire["recovery_only"], true);
    assert_eq!(
        serde_json::from_value::<HostCommand>(wire).expect("decode continue command"),
        command
    );

    let command = HostCommand::ContinueConversation {
        version: HOST_COMMAND_VERSION,
        request_id: "desktop-2".into(),
        source_ref: SOURCE_REF.into(),
        conversation_ref: format!("{RUNTIME_REF}/conversations/one"),
        author_actor_ref: crate::v2026_09::test_fixture::actor_ref("person").into(),
        text: "Continue normally".into(),
        recovery_only: false,
    };
    let wire = serde_json::to_value(&command).expect("serialize ordinary continue command");
    assert_eq!(wire.get("recovery_only"), None);
    assert_eq!(
        serde_json::from_value::<HostCommand>(wire).expect("decode ordinary continue command"),
        command
    );

    let action = HostCommand::InvokeAction {
        version: HOST_COMMAND_VERSION,
        request_id: "desktop-action".into(),
        source_ref: Some(SOURCE_REF.into()),
        author_actor_ref: crate::v2026_09::test_fixture::actor_ref("person").into(),
        conversation_ref: None,
        activation: "@helper--draft--brief".into(),
        input: serde_json::json!({"topic": "resident ownership"}),
    };
    action.validate().expect("validate action command");
    let wire = serde_json::to_value(&action).expect("serialize action command");
    assert_eq!(wire["kind"], "invoke_action");
    assert_eq!(
        serde_json::from_value::<HostCommand>(wire).expect("decode action command"),
        action
    );

    let stop = HostCommand::StopConversation {
        version: HOST_COMMAND_VERSION,
        request_id: "desktop-2".into(),
        source_ref: SOURCE_REF.into(),
        conversation_ref: format!("{RUNTIME_REF}/conversations/one"),
        requesting_actor_ref: crate::v2026_09::test_fixture::actor_ref("person").into(),
        invocation_ref: None,
    };
    stop.validate().expect("validate stop command");
    let wire = serde_json::to_value(&stop).expect("serialize stop command");
    assert_eq!(wire["kind"], "stop_conversation");
    assert_eq!(
        serde_json::from_value::<HostCommand>(wire).expect("decode stop command"),
        stop
    );

    let event = HostEvent::CommandSucceeded {
        request_id: "desktop-1".into(),
        receipt: HostCommandReceipt {
            attachment_ref: None,
            conversation_ref: format!("{RUNTIME_REF}/conversations/one"),
            message_ref: Some(format!("{RUNTIME_REF}/messages/two")),
            invocation_ref: None,
            stopped_invocation_refs: Vec::new(),
        },
    };
    event.validate().expect("validate command receipt");
}
