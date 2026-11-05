use super::*;
use cwr::v2026_09::upstream::{UpstreamAction, UpstreamCommand, UpstreamCoworker, UpstreamEvent};
use serde_json::json;

#[test]
fn host_stdio_routes_upstream_work_and_keeps_the_result_in_its_requesting_conversation() {
    let temporary = tempfile::tempdir().unwrap();
    let (_child, mut input, receiver) = start_runtime_with_configuration(temporary.path(), "{}");
    let actor = "cwp://example.test/tuis/one/actors/tui";
    let registered = send(
        &mut input,
        &receiver,
        HostCommand::Upstream {
            version: HOST_COMMAND_VERSION,
            request_id: "register".into(),
            command: UpstreamCommand::Register {
                coworkers: vec![UpstreamCoworker {
                    actor_ref: actor.into(),
                    coworker_ref: "cwp://example.test/2026-09/coworkers/tui".into(),
                    handle: "@tui".into(),
                    name: "TUI".into(),
                    actions: vec![UpstreamAction {
                        action_name: "new--conversation".into(),
                        title: "New".into(),
                        description: "New".into(),
                        input_schema: json!({"type":"object"}),
                        output_schema: json!({"type":"object"}),
                    }],
                }],
            },
        },
    );
    let HostEvent::UpstreamRegistered {
        provider_session_ref,
        ..
    } = registered
    else {
        panic!("registration failed: {registered:?}");
    };
    let enabled = send(
        &mut input,
        &receiver,
        HostCommand::Upstream {
            version: HOST_COMMAND_VERSION,
            request_id: "enable".into(),
            command: UpstreamCommand::Enable {
                provider_session_ref,
                actor_ref: actor.into(),
                enabled: true,
            },
        },
    );
    assert!(matches!(enabled, HostEvent::CommandSucceeded { .. }));
    let admitted = send(
        &mut input,
        &receiver,
        HostCommand::CreateConversation {
            version: HOST_COMMAND_VERSION,
            request_id: "new".into(),
            source_ref: SOURCE_REF.into(),
            author_actor_ref: crate::test_refs::actor_ref("person").as_str().into(),
            text: "@tui new".into(),
        },
    );
    let HostEvent::CommandSucceeded { receipt, .. } = admitted else {
        panic!("admission failed: {admitted:?}");
    };
    let work = loop {
        if let HostEvent::Upstream {
            event: UpstreamEvent::WorkRequested { work },
        } = receive(&receiver)
        {
            break work;
        }
    };
    assert_eq!(work.conversation_ref, receipt.conversation_ref);
    assert_eq!(work.addressed_message.unwrap().text, "new");
    let protection = send(
        &mut input,
        &receiver,
        HostCommand::Upstream {
            version: HOST_COMMAND_VERSION,
            request_id: "protect".into(),
            command: UpstreamCommand::ProtectResult {
                invocation_ref: work.invocation_ref.clone(),
            },
        },
    );
    assert!(matches!(protection, HostEvent::CommandSucceeded { .. }));
    let completion = HostCommand::Upstream {
        version: HOST_COMMAND_VERSION,
        request_id: "complete".into(),
        command: UpstreamCommand::Complete {
            invocation_ref: work.invocation_ref.clone(),
            terminal: json!({"invocation_ref":work.invocation_ref, "sequence":1,"status":200,"summary":"Starting a new conversation.","result":{"text":"Starting a new conversation."}}),
        },
    };
    serde_json::to_writer(&mut input, &completion).unwrap();
    input.write_all(b"\n").unwrap();
    input.flush().unwrap();
    let mut acknowledged = false;
    let mut retained = false;
    let mut validated = false;
    while !acknowledged || !retained {
        let event = receive(&receiver);
        if matches!(&event, HostEvent::CommandSucceeded { request_id, .. } if request_id == "complete")
        {
            acknowledged = true;
        }
        if let HostEvent::Upstream {
            event:
                UpstreamEvent::ValidateResult {
                    invocation_ref,
                    validation_ref,
                },
        } = &event
        {
            assert_eq!(invocation_ref, &work.invocation_ref);
            let response = HostCommand::Upstream {
                version: HOST_COMMAND_VERSION,
                request_id: "validated".into(),
                command: UpstreamCommand::ValidateResult {
                    validation_ref: validation_ref.clone(),
                    allowed: true,
                    authority_guards: Vec::new(),
                },
            };
            serde_json::to_writer(&mut input, &response).unwrap();
            input.write_all(b"\n").unwrap();
            input.flush().unwrap();
            validated = true;
        }
        if let HostEvent::ConversationUpsert { conversation, .. } = event {
            if conversation.messages.iter().any(|message| {
                message.author_actor_ref == actor && message.text == "Starting a new conversation."
            }) {
                assert_eq!(conversation.conversation_ref, receipt.conversation_ref);
                assert!(
                    validated,
                    "answer must wait for resource-owner delivery validation"
                );
                retained = true;
            }
        }
    }
    let next = send(
        &mut input,
        &receiver,
        HostCommand::CreateConversation {
            version: HOST_COMMAND_VERSION,
            request_id: "revoke-new".into(),
            source_ref: SOURCE_REF.into(),
            author_actor_ref: crate::test_refs::actor_ref("person").as_str().into(),
            text: "@tui new".into(),
        },
    );
    assert!(matches!(next, HostEvent::CommandSucceeded { .. }));
    let work = loop {
        if let HostEvent::Upstream {
            event: UpstreamEvent::WorkRequested { work },
        } = receive(&receiver)
        {
            break work;
        }
    };
    let revoke = HostCommand::Upstream {
        version: HOST_COMMAND_VERSION,
        request_id: "revoke".into(),
        command: UpstreamCommand::RevokeResult {
            invocation_ref: work.invocation_ref.clone(),
        },
    };
    serde_json::to_writer(&mut input, &revoke).unwrap();
    input.write_all(b"\n").unwrap();
    input.flush().unwrap();
    let mut stop_acknowledged = false;
    loop {
        let event = receive(&receiver);
        if let HostEvent::Upstream {
            event: UpstreamEvent::StopRequested { invocation_ref },
        } = &event
        {
            assert_eq!(invocation_ref, &work.invocation_ref);
            let ack = HostCommand::Upstream {
                version: HOST_COMMAND_VERSION,
                request_id: "stop-ack".into(),
                command: UpstreamCommand::AcknowledgeStop {
                    invocation_ref: invocation_ref.clone(),
                },
            };
            serde_json::to_writer(&mut input, &ack).unwrap();
            input.write_all(b"\n").unwrap();
            input.flush().unwrap();
            stop_acknowledged = true;
        }
        if matches!(&event,HostEvent::CommandSucceeded {request_id,..} if request_id == "revoke") {
            assert!(
                stop_acknowledged,
                "revocation must leave the reader free for stop acknowledgement"
            );
            break;
        }
    }
}
