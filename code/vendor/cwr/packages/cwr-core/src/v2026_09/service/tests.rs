use super::*;
use crate::intrinsic_cwr::IntrinsicCwrCommand;
use crate::open_responses::v2026_04_24::AssistantPhase;

#[test]
fn service_commands_are_strict_and_versioned() {
    let command = RuntimeServiceCommand::SubmitText {
        version: RUNTIME_SERVICE_VERSION,
        request_id: "desktop-1".into(),
        integration_ref: "cwp://example.test/integrations/desktop".into(),
        author_actor_ref: "~@/actors/person".into(),
        conversation_ref: None,
        text: "@helper begin".into(),
    };
    let value = serde_json::to_value(&command).expect("encode service command");
    assert_eq!(value["kind"], "submit_text");
    assert_eq!(
        serde_json::from_value::<RuntimeServiceCommand>(value).expect("decode service command"),
        command
    );
    assert_eq!(command.request_id(), "desktop-1");
    assert_eq!(command.version(), RUNTIME_SERVICE_VERSION);

    let intrinsic = RuntimeServiceCommand::ExecuteIntrinsic {
        version: RUNTIME_SERVICE_VERSION,
        request_id: "slack-intrinsic-1".into(),
        operation_id: None,
        command: IntrinsicCwrCommand::ListCoworkers,
        requesting_actor_ref: "~@/actors/person".into(),
        conversation_ref: None,
    };
    let value = serde_json::to_value(&intrinsic).expect("encode intrinsic command");
    assert_eq!(value["kind"], "execute_intrinsic");
    assert_eq!(
        serde_json::from_value::<RuntimeServiceCommand>(value).expect("decode intrinsic command"),
        intrinsic
    );
}

#[test]
fn gateway_inspection_wire_shape_is_versioned_and_exact() {
    let command = RuntimeServiceCommand::InspectGatewayInvocation {
        version: RUNTIME_SERVICE_VERSION,
        request_id: "inspect-1".into(),
        connection_ref: "cwp://example.test/connections/sms".into(),
        invocation_ref: "cwp://example.test/invocations/one".into(),
        invocation_kind: GatewayInvocationKind::Delivery,
        input: serde_json::json!({"delivery_ref":"one"}),
    };
    let encoded = serde_json::to_value(&command).unwrap();
    assert_eq!(encoded["kind"], "inspect_gateway_invocation");
    assert_eq!(encoded["invocation_kind"], "delivery");
    assert_eq!(
        serde_json::from_value::<RuntimeServiceCommand>(encoded).unwrap(),
        command
    );
    let event = RuntimeServiceEvent::GatewayInvocationInspected {
        request_id: "inspect-1".into(),
        authorized: false,
        terminal: None,
    };
    assert_eq!(serde_json::to_value(event).unwrap()["authorized"], false);
    let replayed = RuntimeServiceEvent::GatewayInvocationInspected {
        request_id: "inspect-2".into(),
        authorized: true,
        terminal: Some(GatewayInvocationTerminal {
            status: 200,
            result: Some(serde_json::json!({"delivered": true})),
        }),
    };
    let value = serde_json::to_value(&replayed).unwrap();
    assert_eq!(value["terminal"]["status"], 200);
    assert_eq!(
        serde_json::from_value::<RuntimeServiceEvent>(value).unwrap(),
        replayed
    );
}

#[test]
fn ready_events_keep_the_legacy_wire_shape() {
    #[derive(Deserialize)]
    #[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
    enum LegacyRuntimeServiceEvent {
        Ready { runtime_ref: String },
    }

    let value = serde_json::to_value(RuntimeServiceEvent::Ready {
        runtime_ref: "cwp://example.test/runtime".into(),
    })
    .expect("encode current ready event");
    assert_eq!(
        value,
        serde_json::json!({
            "kind": "ready",
            "runtime_ref": "cwp://example.test/runtime"
        })
    );
    let LegacyRuntimeServiceEvent::Ready { runtime_ref } =
        serde_json::from_value(value).expect("legacy reader accepts current ready event");
    assert_eq!(runtime_ref, "cwp://example.test/runtime");
}

#[test]
fn service_stop_accepts_existing_clients_and_preserves_provider_event_time() {
    let mut value = serde_json::json!({
        "kind": "stop_conversation",
        "version": RUNTIME_SERVICE_VERSION,
        "request_id": "stop-event",
        "conversation_ref": "cwp://example.test/runtime/conversations/one",
        "requesting_actor_ref": "~@/actors/person"
    });
    let command: RuntimeServiceCommand = serde_json::from_value(value.clone()).unwrap();
    assert!(matches!(
        command,
        RuntimeServiceCommand::StopConversation {
            event_at_unix_ms: None,
            ..
        }
    ));
    assert_eq!(serde_json::to_value(command).unwrap(), value);
    value["event_at_unix_ms"] = serde_json::json!(123_456);
    let command: RuntimeServiceCommand = serde_json::from_value(value.clone()).unwrap();
    assert!(matches!(
        command,
        RuntimeServiceCommand::StopConversation {
            event_at_unix_ms: Some(123_456),
            ..
        }
    ));
    assert_eq!(serde_json::to_value(command).unwrap(), value);
}

#[test]
fn service_live_output_is_provider_neutral_and_clearable() {
    let event = RuntimeServiceEvent::ConversationLiveOutput {
        conversation_ref: "cwp://example.test/runtime/conversations/one".into(),
        output: Some(ConversationLiveOutput {
            invocation_ref: "cwp://example.test/runtime/invocations/one".into(),
            generation: 1,
            target_actor_ref: "~@/actors/helper".into(),
            phase: AssistantPhase::Commentary,
            text: "Checking the workspace".into(),
        }),
    };
    let value = serde_json::to_value(&event).expect("encode live output");
    assert_eq!(value["kind"], "conversation_live_output");
    assert!(value["output"].get("provider").is_none());
    assert_eq!(
        serde_json::from_value::<RuntimeServiceEvent>(value).expect("decode live output"),
        event
    );

    let cleared = RuntimeServiceEvent::ConversationLiveOutput {
        conversation_ref: "cwp://example.test/runtime/conversations/one".into(),
        output: None,
    };
    let value = serde_json::to_value(&cleared).expect("encode cleared live output");
    assert!(value.get("output").is_none());
    assert_eq!(
        serde_json::from_value::<RuntimeServiceEvent>(value).expect("decode cleared output"),
        cleared
    );
}

#[test]
fn catalog_updates_round_trip_with_the_selected_actor() {
    let event = RuntimeServiceEvent::CatalogUpdated {
        catalog: LocalCatalog {
            default_conversation_target_ref: None,
            acting_actor: crate::v2026_09::local::LocalActorIdentity {
                actor_ref: "~@/actors/person".into(),
                coworker_ref: "~@/coworkers/person".into(),
                handle: "@person".into(),
                name: "Person".into(),
                category: None,
                provider_model: None,
            },
            known_actors: Vec::new(),
            retained_actors: Vec::new(),
            actors: Vec::new(),
            actions: Vec::new(),
        },
    };

    let value = serde_json::to_value(&event).expect("encode catalog update");
    assert_eq!(value["kind"], "catalog_updated");
    assert_eq!(
        value["catalog"]["acting_actor"]["actor_ref"],
        "~@/actors/person"
    );
    assert_eq!(
        serde_json::from_value::<RuntimeServiceEvent>(value).expect("decode catalog update"),
        event
    );
}

#[test]
fn delivery_diagnostic_does_not_disable_retries() {
    let command: RuntimeServiceCommand = serde_json::from_value(serde_json::json!({
        "kind": "delivery_failed", "version": RUNTIME_SERVICE_VERSION,
        "request_id": "failure", "attachment_ref": "attachment", "delivery_ref": "delivery",
        "error": "{\"outcome\":\"permanent\"}"
    }))
    .unwrap();
    assert!(matches!(
        command,
        RuntimeServiceCommand::DeliveryFailed {
            permanent: false,
            retry_after: None,
            ..
        }
    ));
}
